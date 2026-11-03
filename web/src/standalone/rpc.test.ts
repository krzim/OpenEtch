// The page's transport (rpc.ts) against the real worker loop (host.ts) run in Node: Responses as the fetch wrappers
// read them, the event stream, the session id across a reload, a panic restarting the worker with the recovery
// notice, a stuck worker, and a build that cannot start.

import { test } from "node:test";
import assert from "node:assert/strict";
import type { FromWorker, Notice, ToWorker, WebcoreBackend } from "./contract.ts";
import { workerHost } from "./host.ts";
import { MemoryStore } from "./idb.ts";
import { MockCore } from "./mockCore.ts";
import { clock, FakeSocket, SESSION_KEY, splitPath, StandaloneClient, type SessionStore, type WorkerLike } from "./rpc.ts";

/** A Worker whose other end is host.ts in this process (messages cloned with their transfers, delivered later). */
class InProcessWorker implements WorkerLike {
  onmessage: ((e: { data: FromWorker }) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  terminated = false;
  /** Requests are swallowed (a worker stuck in webcore). */
  stuck = false;
  readonly cores: MockCore[];
  private readonly host: (m: ToWorker) => void;

  constructor(store: MemoryStore, cores: MockCore[], loadCore?: () => Promise<WebcoreBackend>) {
    this.cores = cores;
    this.host = workerHost({
      post: (m, transfer = []) => {
        const copy = structuredClone(m, { transfer });
        setTimeout(() => {
          if (!this.terminated) this.onmessage?.({ data: copy });
        }, 0);
      },
      openStore: async () => store,
      loadCore:
        loadCore ??
        (async () => {
          const c = new MockCore();
          cores.push(c);
          return c;
        }),
      channel: () => null,
      every: () => undefined,
    });
  }

  postMessage(m: ToWorker, transfer: Transferable[] = []): void {
    const copy = structuredClone(m, { transfer });
    if (this.stuck && m.kind === "request") return;
    setTimeout(() => {
      if (!this.terminated) this.host(copy);
    }, 0);
  }

  terminate(): void {
    this.terminated = true;
  }
}

function memorySession(): SessionStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return { map, getItem: (k) => map.get(k) ?? null, setItem: (k, v) => void map.set(k, v) };
}

function setup(opts: { store?: MemoryStore; session?: ReturnType<typeof memorySession>; loadCore?: () => Promise<WebcoreBackend>; timeoutMs?: number } = {}) {
  const store = opts.store ?? new MemoryStore();
  const cores: MockCore[] = [];
  const workers: InProcessWorker[] = [];
  const notices: Notice[] = [];
  const session = opts.session ?? memorySession();
  let persisted = 0;
  const client = new StandaloneClient({
    createWorker: () => {
      const w = new InProcessWorker(store, cores, opts.loadCore);
      workers.push(w);
      return w;
    },
    session,
    timeoutMs: opts.timeoutMs,
    persist: async () => {
      persisted++;
      return true;
    },
    newId: () => "session-1",
  });
  // The in-memory store says so once; the tests look at the other notices.
  let memoryNotices = 0;
  client.onNotice((n) => (n.key === "ds.storageMemory" ? memoryNotices++ : notices.push(n)));
  return { client, store, cores, workers, notices, session, persisted: () => persisted, memoryNotices: () => memoryNotices };
}

const tick = () => new Promise((r) => setTimeout(r, 5));

test("requests come back as Responses, with status, headers and bodies the wrappers read", async () => {
  const { client } = setup();
  const r = await client.fetch("GET", "/api/project");
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("content-type"), "application/json");
  assert.equal((await r.json()).dirty, false);

  const ex = await client.fetch("POST", "/api/execute", { headers: { "Content-Type": "application/json" }, body: JSON.stringify({ id: "shape.rect" }) });
  assert.equal((await ex.json()).ok, true);

  const dl = await client.fetch("GET", "/api/document/download?format=openetch");
  assert.match(dl.headers.get("content-disposition") ?? "", /Untitled\.openetch/);
  const blob = await dl.blob();
  assert.ok(blob.size > 10);

  // A Blob body (an upload) and a 4xx with the engine's JSON error.
  const up = await client.fetch("POST", "/api/document/upload?format=svg", { body: new Blob(["<svg/>"]) });
  assert.equal(up.status, 200);
  const missing = await client.fetch("POST", "/api/projects/open", { body: JSON.stringify({ name: "Nope" }) });
  assert.equal(missing.status, 404);
  assert.equal((await missing.json()).ok, false);
  // A full URL works as a path; internal routes are refused here already.
  assert.equal((await client.fetch("GET", "http://localhost/api/machine")).status, 200);
  assert.equal((await client.fetch("POST", "/api/_shell/project_saved")).status, 404);
  client.terminate();
});

test("the first successful save asks for persistent storage, once", async () => {
  const s = setup();
  await s.client.fetch("POST", "/api/projects/save", { body: JSON.stringify({ name: "One" }) });
  await s.client.fetch("POST", "/api/projects/save", { body: JSON.stringify({ name: "Two" }) });
  assert.equal(s.persisted(), 1);
  s.client.terminate();
});

test("events reach the FakeSocket, which opens once the worker is ready", async () => {
  const { client } = setup();
  const sock = client.events();
  assert.equal(sock.readyState, FakeSocket.CONNECTING);
  const got: string[] = [];
  let opened = 0;
  sock.onopen = () => opened++;
  sock.onmessage = (m) => got.push(JSON.parse(m.data).type);
  await client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.rect" }) });
  await tick();
  assert.equal(opened, 1);
  assert.equal(sock.readyState, FakeSocket.OPEN);
  assert.deepEqual(got, ["document_changed"]);
  let closed = 0;
  sock.onclose = () => closed++;
  sock.close();
  await client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.rect" }) });
  await tick();
  assert.equal(got.length, 1);
  assert.equal(closed, 1);
  client.terminate();
});

test("a reload keeps the session id and brings the unsaved design back, with a notice", async () => {
  const store = new MemoryStore();
  const session = memorySession();
  const first = setup({ store, session });
  await first.client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.rect" }) });
  first.client.flush();
  await tick();
  assert.equal(session.map.get(SESSION_KEY), "session-1");
  first.client.terminate();

  const second = setup({ store, session });
  const doc = await (await second.client.fetch("GET", "/api/document")).json();
  assert.equal(doc.objects.length, 1);
  assert.equal(second.client.lastRecovered?.source, "recovery");
  assert.deepEqual(second.notices.map((n) => n.key), ["ds.reloadRecovered"]);
  assert.match(second.notices[0].vars.time, /^\d\d:\d\d$/);
  second.client.terminate();
});

test("a webcore panic restarts the worker, restores the recovery copy and says so", async () => {
  const s = setup();
  const sock = s.client.events();
  let closed = 0;
  sock.onclose = () => closed++;
  await s.client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.rect" }) });
  await s.client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.ellipse" }) });
  s.client.flush();
  await tick();
  const crash = await s.client.fetch("POST", "/api/_mock/panic");
  assert.equal(crash.status, 500);
  assert.equal((await crash.json()).code, "editor_crashed");
  // Sent while the new worker starts: answered by it.
  const doc = await (await s.client.fetch("GET", "/api/document")).json();
  assert.equal(doc.objects.length, 2);
  assert.equal(s.workers.length, 2);
  assert.ok(s.workers[0].terminated);
  assert.equal(closed, 1);
  assert.equal(sock.readyState, FakeSocket.CLOSED);
  assert.deepEqual(s.notices.map((n) => n.key), ["ds.restartedRecovered"]);
  assert.equal(s.notices[0].vars.time, clock(s.client.lastRecovered!.savedMs));
  // A new stream opens on the new worker.
  const again = s.client.events();
  await tick();
  assert.equal(again.readyState, FakeSocket.OPEN);
  s.client.terminate();
});

test("a crash before any recovery copy: restarted, and the notice says nothing was recovered", async () => {
  const s = setup();
  await s.client.fetch("POST", "/api/_mock/panic");
  await s.client.fetch("GET", "/api/project");
  assert.deepEqual(s.notices.map((n) => n.key), ["ds.restartedNothing"]);
  // The memory-storage notice came once, not again after the restart.
  assert.equal(s.memoryNotices(), 1);
  s.client.terminate();
});

test("a recovery copy that makes webcore panic is dropped: the next worker starts clean", async () => {
  const store = new MemoryStore();
  const session = memorySession();
  const first = setup({ store, session });
  await first.client.fetch("POST", "/api/execute", { body: JSON.stringify({ id: "shape.rect" }) });
  first.client.flush();
  await first.client.fetch("GET", "/api/project");
  first.client.terminate();
  let n = 0;
  const second = setup({
    store,
    session,
    loadCore: async () => {
      const c = new MockCore();
      if (++n === 1) c.panicOn = "/api/_shell/recover";
      return c;
    },
  });
  const r = await second.client.fetch("GET", "/api/document");
  assert.equal(r.status, 200);
  assert.equal((await r.json()).objects.length, 0);
  assert.equal(second.workers.length, 2);
  assert.deepEqual(second.notices.map((x) => x.key), ["ds.restartedNothing"]);
  second.client.terminate();
});

test("more than three crashes within a minute: the page stops restarting and says so", async () => {
  const s = setup();
  for (let i = 0; i < 4; i++) await s.client.fetch("POST", "/api/_mock/panic");
  const r = await s.client.fetch("GET", "/api/project");
  assert.equal(r.status, 503);
  assert.equal((await r.json()).code, "editor_failed");
  assert.equal(s.workers.length, 4);
  assert.equal(s.notices.at(-1)?.key, "ds.failed");
});

test("a worker that does not answer in time is replaced (504)", async () => {
  const s = setup({ timeoutMs: 50 });
  // The first worker is stuck (webcore in an endless loop): it never answers a request again.
  await s.client.fetch("GET", "/api/project");
  s.workers[0].stuck = true;
  const r = await s.client.fetch("GET", "/api/project");
  assert.equal(r.status, 504);
  assert.equal((await r.json()).code, "editor_timeout");
  const ok = await s.client.fetch("GET", "/api/project");
  assert.equal(ok.status, 200);
  assert.equal(s.workers.length, 2);
  assert.ok(s.workers[0].terminated);
  s.client.terminate();
});

test("a build whose core cannot load fails once, with a notice, and answers 503", async () => {
  const s = setup({ loadCore: async () => Promise.reject(new Error("no wasm")) });
  const r = await s.client.fetch("GET", "/api/project");
  assert.equal(r.status, 503);
  assert.equal((await r.json()).code, "editor_failed");
  assert.equal(s.workers.length, 1);
  assert.deepEqual(s.notices.map((n) => [n.key, n.vars.error]), [["ds.failed", "no wasm"]]);
  const sock = s.client.events();
  await tick();
  assert.equal(sock.readyState, FakeSocket.CLOSED);
});

test("an aborted request rejects with AbortError, like fetch", async () => {
  const s = setup();
  await s.client.fetch("GET", "/api/project");
  const c = new AbortController();
  const p = s.client.fetch("GET", "/api/document", { signal: c.signal });
  c.abort();
  await assert.rejects(p, (e: Error) => e.name === "AbortError");
  const pre = new AbortController();
  pre.abort();
  await assert.rejects(s.client.fetch("GET", "/api/document", { signal: pre.signal }), (e: Error) => e.name === "AbortError");
  assert.equal((await s.client.fetch("GET", "/api/project")).status, 200);
  s.client.terminate();
});

test("splitPath", () => {
  assert.deepEqual(splitPath("/api/x?y=1&z=2"), { path: "/api/x", query: "y=1&z=2" });
  assert.deepEqual(splitPath("/api/x"), { path: "/api/x", query: "" });
  assert.deepEqual(splitPath("https://h.example/sub/api/x?q"), { path: "/sub/api/x", query: "q" });
});
