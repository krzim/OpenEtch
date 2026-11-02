// Unit tests for api/client.ts against a fake fetch: the bearer header on every request, `?token=` only on the
// WebSocket URL, undo/redo `{ok:false}` answers, Stop frame, the approval body and error codes, and the access token
// (401 raises the Connect card, a pasted token reconnects, and the token never goes into a URL).

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
  removeItem: (k: string) => void store.delete(k),
};
g.window = { location: { protocol: "http:", host: "pi.local:7878", href: "http://pi.local:7878/" } };
g.fetch = async (path: string, init: { method: string; headers: Record<string, string>; body?: unknown }) => {
  calls.push({ method: init.method, path, headers: init.headers, body: typeof init.body === "string" ? JSON.parse(init.body) : init.body });
  return {
    ok: reply.status >= 200 && reply.status < 300,
    status: reply.status,
    statusText: "",
    text: async () => JSON.stringify(reply.body),
  };
};

const client = await import("./client.ts");
const { api, ApiError, eventsUrl } = client;

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

test("approve without a PIN (no-PIN mode) sends an empty body", async () => {
  await api.approveRun("run-2", null);
  assert.equal(calls[0]?.path, "/api/runs/run-2/approve");
  assert.deepEqual(calls[0]?.body, {});
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

test("a 401 raises the Connect card (needsToken) and nothing else does", async () => {
  client.markAuthorized();
  let changes = 0;
  const off = client.subscribeNeedsToken(() => void changes++);
  reply = { status: 403, body: { ok: false, error: "origin not allowed" } };
  await assert.rejects(api.machine(), (e: unknown) => e instanceof ApiError && e.status === 403);
  reply = { status: 500, body: { ok: false, error: "boom" } };
  await assert.rejects(api.document());
  assert.equal(client.needsTokenNow(), false);
  reply = { status: 401, body: { ok: false, error: "missing or wrong access token" } };
  await assert.rejects(api.machine(), (e: unknown) => e instanceof ApiError && e.status === 401);
  assert.equal(client.needsTokenNow(), true);
  // Every route reports it, uploads and downloads included.
  client.markAuthorized();
  await assert.rejects(api.download("svg"));
  assert.equal(client.needsTokenNow(), true);
  client.markAuthorized();
  await assert.rejects(api.upload(new Blob(["x"]), "svg"));
  assert.equal(client.needsTokenNow(), true);
  off();
  assert.ok(changes >= 2);
  client.markAuthorized();
});

test("a pasted token is checked, kept on success and forgotten when refused; never in a URL", async () => {
  client.markUnauthorized();
  reply = { status: 401, body: { ok: false, error: "missing or wrong access token" } };
  const refused = await client.tryToken("wrong-token-0123456789");
  assert.deepEqual(refused.ok ? null : refused.reason, "refused");
  assert.equal(client.needsTokenNow(), true);
  assert.equal(store.has("openetch.token"), false, "a refused token is not kept");
  assert.equal(calls.at(-1)?.headers.Authorization, "Bearer wrong-token-0123456789");

  reply = { status: 200, body: { connected: true } };
  assert.deepEqual(await client.tryToken("good-token-0123456789"), { ok: true });
  assert.equal(client.needsTokenNow(), false, "accepted: the app comes back without a reload");
  assert.equal(store.get("openetch.token"), "good-token-0123456789");
  await api.machine();
  assert.equal(calls.at(-1)?.headers.Authorization, "Bearer good-token-0123456789");

  // "Remember on this device" off: this tab only, nothing stored.
  assert.deepEqual(await client.tryToken("tab-token-0123456789", false), { ok: true });
  assert.equal(store.has("openetch.token"), false);
  assert.equal(client.authToken(), "tab-token-0123456789");
  assert.equal(eventsUrl(), "ws://pi.local:7878/api/events?token=tab-token-0123456789");

  // The engine down is not a refusal.
  const fetchOk = g.fetch;
  g.fetch = async () => {
    throw new Error("connection refused");
  };
  const down = await client.tryToken(null);
  assert.deepEqual(down.ok ? null : down.reason, "unreachable");
  g.fetch = fetchOk;

  for (const c of calls) {
    assert.ok(!c.path.includes("token"), `no token in ${c.path}`);
  }
  client.setToken("tok-123");
  assert.equal(client.authToken(), "tok-123");
  assert.deepEqual([...store.keys()], ["openetch.token"]);
});

test("tokenFromInput takes a token, a pairing URL or a line of `openetch token`", () => {
  const t = "0123456789abcdef0123456789abcdef";
  assert.equal(client.tokenFromInput(`  ${t}
`), t);
  assert.equal(client.tokenFromInput(`http://192.168.1.20:7878/?token=${t}`), t);
  assert.equal(client.tokenFromInput(`openetch: pair a device by opening http://pi:7878/?a=1&token=${t}`), t);
  assert.equal(client.tokenFromInput("short"), null);
  assert.equal(client.tokenFromInput("two words here and more"), null);
  assert.equal(client.tokenFromInput(""), null);
});
