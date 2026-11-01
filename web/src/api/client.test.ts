// Unit tests for api/client.ts against a fake fetch: the bearer header on every request, `?token=` only on the
// WebSocket URL, undo/redo `{ok:false}` answers, Stop frame, and the approval body and error codes.

import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

interface Call {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
}

const calls: Call[] = [];
let reply: { status: number; body: unknown } = { status: 200, body: { ok: true } };
const store = new Map<string, string>([["openetch.token", "tok-123"]]);

const g = globalThis as Record<string, unknown>;
g.localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
};
g.window = { location: { protocol: "http:", host: "pi.local:7878", href: "http://pi.local:7878/" } };
g.fetch = async (path: string, init: { method: string; headers: Record<string, string>; body?: string }) => {
  calls.push({ method: init.method, path, headers: init.headers, body: init.body === undefined ? undefined : JSON.parse(init.body) });
  return {
    ok: reply.status >= 200 && reply.status < 300,
    status: reply.status,
    statusText: "",
    text: async () => JSON.stringify(reply.body),
  };
};

const { api, ApiError, eventsUrl } = await import("./client.ts");

beforeEach(() => {
  calls.length = 0;
  reply = { status: 200, body: { ok: true } };
});

test("every request carries the bearer header and no ?token=", async () => {
  await api.machine();
  await api.frame();
  await api.frameStop();
  await api.stop();
  for (const c of calls) {
    assert.equal(c.headers.Authorization, "Bearer tok-123");
    assert.ok(!c.path.includes("token"), c.path);
  }
});

test("the WebSocket URL carries ?token=", () => {
  assert.equal(eventsUrl(), "ws://pi.local:7878/api/events?token=tok-123");
});

test("Stop frame ends framing only; STOP is the emergency endpoint", async () => {
  await api.frameStop();
  await api.stop();
  assert.deepEqual(
    calls.map((c) => `${c.method} ${c.path}`),
    ["POST /api/machine/frame/stop", "POST /api/machine/stop"],
  );
});

test("undo/redo throw on 200 {ok:false}", async () => {
  reply = { status: 200, body: { ok: false, error: "nothing to undo" } };
  await assert.rejects(api.undo(), (e: unknown) => e instanceof ApiError && e.message === "nothing to undo");
  reply = { status: 200, body: { ok: false } };
  await assert.rejects(api.redo(), /nothing to redo/);
  reply = { status: 200, body: { ok: true, revision: 4 } };
  await api.undo();
});

test("approve sends only the PIN and surfaces the error code", async () => {
  await api.approveRun("run/1", "4321");
  assert.equal(calls[0]?.path, "/api/runs/run%2F1/approve");
  assert.deepEqual(calls[0]?.body, { pin: "4321" });

  reply = { status: 403, body: { ok: false, error: "wrong PIN", code: "pin_invalid" } };
  await assert.rejects(api.approveRun("run-1", "0000"), (e: unknown) => {
    assert.ok(e instanceof ApiError);
    assert.equal(e.status, 403);
    assert.equal(e.code, "pin_invalid");
    assert.equal(e.detail, "wrong PIN");
    return true;
  });
  // The PIN is never kept anywhere the client can read back.
  assert.deepEqual([...store.keys()], ["openetch.token"]);
});

test("LightBurn routes: GET devices, POST the device list as JSON, DELETE the upload", async () => {
  reply = { status: 200, body: { available: true, source: null, default_device: null, devices: [] } };
  await api.lightburn();
  reply = { status: 200, body: { ok: true } };
  const body = JSON.stringify({ DefaultDevice: 0, DeviceList: [{ Name: "JCZFiber" }] });
  await api.lightburnUpload(body);
  await api.lightburnForget();
  assert.deepEqual(
    calls.map((c) => `${c.method} ${c.path}`),
    ["GET /api/lightburn", "POST /api/lightburn/prefs", "DELETE /api/lightburn/prefs"],
  );
  assert.equal(calls[1]?.headers["Content-Type"], "application/json");
  assert.equal(calls[1]?.headers.Authorization, "Bearer tok-123");
  assert.deepEqual(calls[1]?.body, { DefaultDevice: 0, DeviceList: [{ Name: "JCZFiber" }] });

  reply = { status: 200, body: { ok: false, error: "no devices in DeviceList" } };
  await assert.rejects(api.lightburnUpload(body), /no devices in DeviceList/);
  reply = { status: 413, body: { error: "body larger than 2 MB" } };
  await assert.rejects(api.lightburnUpload(body), (e: unknown) => e instanceof ApiError && e.status === 413 && e.detail === "body larger than 2 MB");
});

test("device routes: list, details, import (file or LightBurn on the engine computer), activate, remove", async () => {
  reply = { status: 200, body: { active: null, devices: [] } };
  await api.devices();
  await api.device("jcz fiber/1");
  reply = { status: 200, body: { added: [], updated: [], skipped: [] } };
  const body = JSON.stringify({ DeviceList: [{ Name: "Custom GCode", DisplayName: "WAZER" }] });
  await api.deviceImport(body, "WAZER & co");
  await api.deviceImport(null, "all");
  reply = { status: 200, body: { ok: true } };
  await api.deviceActivate("wazer");
  await api.deviceRemove("wazer");
  assert.deepEqual(
    calls.map((c) => `${c.method} ${c.path}`),
    [
      "GET /api/devices",
      "GET /api/devices/jcz%20fiber%2F1",
      "POST /api/devices/import?add=WAZER%20%26%20co",
      "POST /api/devices/import?from=lightburn&add=all",
      "POST /api/devices/wazer/activate",
      "DELETE /api/devices/wazer",
    ],
  );
  assert.equal(calls[2]?.headers["Content-Type"], "application/json");
  assert.deepEqual(calls[2]?.body, { DeviceList: [{ Name: "Custom GCode", DisplayName: "WAZER" }] });
  // Reading LightBurn on the engine computer sends no body at all.
  assert.equal(calls[3]?.body, undefined);
  assert.equal(calls[3]?.headers["Content-Type"], undefined);
  for (const c of calls) assert.equal(c.headers.Authorization, "Bearer tok-123");

  // Switching while a run or frame is active: the engine's 409 text reaches the toast.
  reply = { status: 409, body: { ok: false, error: "a run is active" } };
  await assert.rejects(api.deviceActivate("wazer"), (e: unknown) => e instanceof ApiError && e.status === 409 && e.detail === "a run is active");
  reply = { status: 200, body: { ok: false, error: "no devices in DeviceList" } };
  await assert.rejects(api.deviceImport(body, "all"), /no devices in DeviceList/);
});
