import { test } from "node:test";
import assert from "node:assert/strict";
import { FakeSocket, bodyBytes, headerRecord, noMachine, replyResponse, setDesignBackend, splitPath, transport, workerBackend, type WorkerPort } from "./transport.ts";

/** A worker stand-in: records posts; `send` answers as the worker would. */
function fakeWorker() {
  const listeners: { message: ((ev: MessageEvent) => void)[]; error: ((ev: Event) => void)[] } = { message: [], error: [] };
  const posted: { msg: unknown; transfer: Transferable[] }[] = [];
  const port: WorkerPort = {
    postMessage: (msg, transfer) => void posted.push({ msg, transfer }),
    addEventListener: ((type: "message" | "error", fn: (ev: never) => void) => listeners[type].push(fn as never)) as WorkerPort["addEventListener"],
  };
  const send = (data: unknown) => listeners.message.forEach((fn) => fn({ data } as MessageEvent));
  const fail = (message: string) => listeners.error.forEach((fn) => fn({ message } as unknown as Event));
  return { port, posted, send, fail };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

test("splitPath, headerRecord and bodyBytes shape a request for the worker", async () => {
  assert.deepEqual(splitPath("/api/export?format=svg&ids=a"), { path: "/api/export", query: "format=svg&ids=a" });
  assert.deepEqual(splitPath("/api/document"), { path: "/api/document", query: "" });
  assert.deepEqual(headerRecord({ Accept: "application/json", "Content-Type": "x" }), { accept: "application/json", "content-type": "x" });
  assert.deepEqual(headerRecord(undefined), {});
  assert.deepEqual([...(await bodyBytes('{"a":1}'))], [...new TextEncoder().encode('{"a":1}')]);
  assert.equal((await bodyBytes(undefined)).byteLength, 0);
  assert.deepEqual([...(await bodyBytes(new Blob([new Uint8Array([1, 2, 3])])))], [1, 2, 3]);
  // A caller's bytes are copied, so transferring them never detaches the caller's buffer.
  const mine = new Uint8Array([9, 8]);
  const out = await bodyBytes(mine);
  assert.notEqual(out.buffer, mine.buffer);
});

test("replyResponse makes a real Response (headers, status, no body where none is allowed)", async () => {
  const r = replyResponse({ id: 1, status: 200, headers: [["content-disposition", 'attachment; filename="a.svg"']], body: new TextEncoder().encode("<svg/>") });
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("Content-Disposition"), 'attachment; filename="a.svg"');
  assert.equal(await r.text(), "<svg/>");
  const empty = replyResponse({ id: 2, status: 204, headers: {}, body: new Uint8Array([1]) });
  assert.equal(empty.status, 204);
  assert.equal(await empty.text(), "");
});

test("workerBackend matches replies to requests by id and transfers the body", async () => {
  const w = fakeWorker();
  const b = workerBackend(w.port);
  const first = b.request("post", "/api/execute?x=1", { headers: { "Content-Type": "application/json" }, body: '{"id":"shape.rect"}' });
  const second = b.request("GET", "/api/document", {});
  await tick();
  assert.equal(w.posted.length, 2);
  type Req = { id: number; method: string; path: string; query: string; headers: Record<string, string>; body: Uint8Array };
  const [m1, m2] = w.posted.map((p) => p.msg as Req);
  assert.equal(m1.method, "POST");
  assert.equal(m1.path, "/api/execute");
  assert.equal(m1.query, "x=1");
  assert.equal(m1.headers["content-type"], "application/json");
  assert.equal(new TextDecoder().decode(m1.body), '{"id":"shape.rect"}');
  assert.equal(w.posted[0].transfer.length, 1);
  assert.equal(w.posted[1].transfer.length, 0);
  // Answered out of order.
  w.send({ id: m2.id, status: 200, headers: [["content-type", "application/json"]], body: new TextEncoder().encode('{"revision":3}') });
  w.send({ kind: "reply", id: m1.id, status: 422, headers: [], body: new TextEncoder().encode('{"error":"no"}') });
  assert.deepEqual(await (await second).json(), { revision: 3 });
  const r1 = await first;
  assert.equal(r1.status, 422);
  assert.equal(r1.ok, false);
});

test("workerBackend passes events to subscribers and fails waiting requests when the worker dies", async () => {
  const w = fakeWorker();
  const fatal: string[] = [];
  const b = workerBackend(w.port, (m) => fatal.push(m));
  const seen: unknown[] = [];
  const off = b.subscribe((e) => seen.push(e));
  w.send({ kind: "event", data: { type: "document_changed", revision: 4 } });
  off();
  w.send({ kind: "event", data: { type: "project_saved" } });
  assert.deepEqual(seen, [{ type: "document_changed", revision: 4 }]);
  const waiting = b.request("GET", "/api/history", {});
  await tick();
  w.send({ kind: "fatal", message: "wasm panicked" });
  await assert.rejects(waiting, /wasm panicked/);
  assert.deepEqual(fatal, ["wasm panicked"]);
  const again = b.request("GET", "/api/history", {});
  await tick();
  w.fail("worker crashed");
  await assert.rejects(again, /worker crashed/);
});

test("FakeSocket opens on the backend's events and hands them on as JSON text", async () => {
  let listener: ((e: unknown) => void) | null = null;
  let unsubscribed = false;
  setDesignBackend({
    request: async () => new Response("{}"),
    subscribe: (fn) => {
      listener = fn;
      return () => void (unsubscribed = true);
    },
  });
  const s = new FakeSocket();
  const got: string[] = [];
  let opened = false;
  let closed = false;
  s.onopen = () => (opened = true);
  s.onmessage = (m) => void got.push(String(m.data));
  s.onclose = () => (closed = true);
  await tick();
  assert.ok(opened);
  const emit = listener as unknown as (e: unknown) => void;
  emit({ type: "device_changed" });
  emit('{"type":"history_changed"}');
  assert.deepEqual(got, ['{"type":"device_changed"}', '{"type":"history_changed"}']);
  s.close();
  assert.ok(closed && unsubscribed);
  emit({ type: "late" });
  assert.equal(got.length, 2);
});

test("FakeSocket hands over to the backend's own event socket when it has one", async () => {
  const inner = { onopen: null, onmessage: null, onclose: null, onerror: null, closed: false, close() { this.closed = true; } } as unknown as { onopen: ((e: Event) => void) | null; onmessage: ((e: MessageEvent) => void) | null; onclose: ((e: CloseEvent) => void) | null; onerror: null; closed: boolean; close(): void };
  setDesignBackend({ request: async () => new Response("{}"), subscribe: () => () => {}, openEvents: () => inner });
  const s = new FakeSocket();
  const got: string[] = [];
  let closed = 0;
  s.onmessage = (m) => void got.push(String(m.data));
  s.onclose = () => void closed++;
  await tick();
  inner.onopen?.(new Event("open"));
  inner.onmessage?.(new MessageEvent("message", { data: '{"type":"project_saved"}' }));
  assert.deepEqual(got, ['{"type":"project_saved"}']);
  // The backend restarted: its socket closed, so the app's does (useEngine reconnects).
  inner.onclose?.(new Event("close") as CloseEvent);
  assert.equal(closed, 1);
  const t = new FakeSocket();
  await tick();
  t.close();
  assert.ok(inner.closed);
});

test("transport is fetch in the normal build", async () => {
  const seen: [string, RequestInit | undefined][] = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (path: string, init?: RequestInit) => {
    seen.push([path, init]);
    return new Response("ok");
  }) as typeof fetch;
  try {
    const r = await transport("PUT", "/api/prefs", { headers: { A: "b" }, body: "x" });
    assert.equal(await r.text(), "ok");
    assert.equal(seen[0][0], "/api/prefs");
    assert.equal(seen[0][1]?.method, "PUT");
    assert.equal(seen[0][1]?.body, "x");
  } finally {
    globalThis.fetch = real;
  }
});

test("noMachine rejects with a plain explanation", async () => {
  await assert.rejects((noMachine as unknown as () => Promise<never>)(), /no laser/);
});
