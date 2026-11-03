// A page for checking the design-only worker shell (src/standalone/) in real browsers: the real worker, IndexedDB,
// BroadcastChannel, reload and crash recovery, with the mock webcore. Driven by e2e/standalone.mjs (headless Edge
// over CDP, Safari through safaridriver); `window.__ds` is its handle.

import { onStandaloneNotice, standaloneClient } from "../../src/standalone/rpc.ts";
import { canPickFiles, downloadBlob, isAppleTouch } from "../../src/standalone/fsaccess.ts";

const notices = [];
const events = [];
onStandaloneNotice((n) => notices.push(n));
const client = standaloneClient();

function openStream() {
  const s = client.events();
  s.onmessage = (m) => events.push(JSON.parse(m.data).type);
  s.onclose = () => setTimeout(openStream, 50);
}
openStream();

const log = document.getElementById("log");

window.__ds = {
  async fetch(method, path, body) {
    const init = {};
    if (body !== undefined && body !== null) init.body = typeof body === "string" ? body : JSON.stringify(body);
    const r = await client.fetch(method, path, init);
    const ct = r.headers.get("content-type") ?? "";
    const buf = new Uint8Array(await r.arrayBuffer());
    const text = ct.includes("json") ? new TextDecoder().decode(buf) : "";
    return { status: r.status, ct, size: buf.byteLength, text, json: text ? JSON.parse(text) : null };
  },
  /** A request with a body of `n` bytes of an .openetch file (a large upload). */
  async uploadBig(objects) {
    const doc = { format: "openetch", version: 1, name: "Big", objects: Array.from({ length: objects }, (_, i) => ({ id: `o${i}`, kind: "rect", pad: "x".repeat(40) })) };
    const blob = new Blob([JSON.stringify(doc)]);
    const r = await client.fetch("POST", "/api/document/upload?format=openetch&name=Big.openetch", { body: blob });
    return { status: r.status, bytes: blob.size };
  },
  flush: () => client.flush(),
  notices,
  events,
  storage: () => client.storage,
  recovered: () => client.lastRecovered,
  canPick: () => canPickFiles(),
  appleTouch: () => isAppleTouch(),
  download() {
    downloadBlob(new Blob([JSON.stringify({ format: "openetch" })], { type: "application/json" }), "check.openetch");
    return true;
  },
  note(text, ok) {
    const li = document.createElement("li");
    li.className = ok ? "ok" : "bad";
    li.textContent = text;
    log.appendChild(li);
  },
};

// Ready once the worker answers.
client.fetch("GET", "/api/project").then((r) => {
  document.getElementById("status").textContent = r.ok ? "ready" : `error ${r.status}`;
  document.getElementById("storage").textContent = client.storage ?? "?";
  window.__dsReady = r.ok;
});
