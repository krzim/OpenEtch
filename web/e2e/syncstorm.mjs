// Change-set storm against a local simulator engine (never the Pi: it edits and deletes freely): concurrent edits,
// undo/redo storms, timeline moves, edits without the sync header (another tab or MCP), document switches. The client
// copy follows the payloads with the web app's own syncStep (src/api/docSync.ts) and is compared with
// GET /api/document after every round. `node e2e/syncstorm.mjs <engine url> [rounds] [seed]`; exits 1 on a mismatch.
import { syncStep } from "../src/api/docSync.ts";

const BASE = process.argv[2] ?? "http://127.0.0.1:7951";
const ROUNDS = Number(process.argv[3] ?? 40);
let seed = Number(process.argv[4] ?? 1);
const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
const pick = (a) => a[Math.floor(rnd() * a.length)];

let held = null;
let epoch = null;
let stats = { apply: 0, replace: 0, same: 0, gap: 0, refetch: 0 };

function canon(v) {
  if (Array.isArray(v)) return v.map(canon);
  if (v && typeof v === "object") return Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])]));
  return v;
}

function take(p) {
  const s = syncStep(held, epoch, p);
  stats[s.kind]++;
  if (s.kind === "gap") return false;
  if (s.kind === "apply" || s.kind === "replace") {
    held = s.doc;
    epoch = p.epoch;
  }
  return true;
}

async function sync() {
  const q = held && epoch ? `?since=${held.revision}&epoch=${epoch}` : "";
  stats.refetch++;
  const p = (await (await fetch(`${BASE}/api/document/sync${q}`)).json());
  if (!take(p)) take((await (await fetch(`${BASE}/api/document/sync`)).json()));
}

async function post(path, body, withHeader = true) {
  const headers = { "content-type": "application/json" };
  if (withHeader && held && epoch) headers["X-OpenEtch-Since"] = `${epoch}:${held.revision}`;
  const r = await fetch(BASE + path, { method: "POST", headers, body: JSON.stringify(body) });
  const t = await r.text();
  let j = null;
  try {
    j = JSON.parse(t);
  } catch {}
  if (withHeader && j && j.sync) {
    if (!take(j.sync)) await sync();
  }
  return j;
}

const exec = (id, params, h = true) => post("/api/execute", { id, params, origin: "ui" }, h);

function ids() {
  return (held?.objects ?? []).map((o) => o.id);
}

async function op() {
  const r = rnd();
  const h = rnd() > 0.2; // 20%: another tab / MCP (no header; this client learns it from an "event" = sync later)
  const os = ids();
  if (r < 0.25 || os.length < 3) await exec("shape.rect", { width_mm: 1 + rnd() * 5, height_mm: 2, center: [rnd() * 40 - 20, rnd() * 40 - 20] }, h);
  else if (r < 0.45) await exec("object.move", { ids: [pick(os)], from_point: [0, 0], to_point: [rnd(), rnd()] }, h);
  else if (r < 0.55) await exec("object.delete", { ids: [pick(os)] }, h);
  else if (r < 0.62) await exec("layer.update", { id: held.layers[0].id, settings: { speed_mm_s: 100 + Math.floor(rnd() * 1000) } }, h);
  else if (r < 0.75) await post("/api/undo", { origin: "ui" }, h);
  else if (r < 0.85) await post("/api/redo", { origin: "ui" }, h);
  else if (r < 0.92) {
    const hist = await (await fetch(BASE + "/api/history")).json();
    const n = Array.isArray(hist.steps) ? hist.steps.length : 0;
    await post("/api/history/goto", { marker: Math.floor(rnd() * (n + 1)), origin: "ui" }, h);
  } else if (r < 0.96) await post("/api/history/delete", { index: 1, origin: "ui" }, h);
  else {
    // Switch documents: open a new one (activated), then sometimes back to the default.
    const d = await post("/api/documents", { origin: "ui" }, h);
    if (rnd() < 0.7) await post(`/api/documents/default/activate`, { origin: "ui" }, h);
    if (d && d.id && rnd() < 0.8) await post(`/api/documents/${d.id}/close`, { discard: true }, h);
  }
}

let bad = 0;
await sync();
for (let round = 0; round < ROUNDS; round++) {
  const k = 1 + Math.floor(rnd() * 6);
  await Promise.all(Array.from({ length: k }, () => op().catch((e) => console.log("op error", String(e)))));
  await sync(); // the document_changed event's fetch
  const truth = await (await fetch(BASE + "/api/document")).json();
  const a = JSON.stringify(canon(held));
  const b = JSON.stringify(canon(truth));
  if (a !== b) {
    bad++;
    console.log(`round ${round}: MISMATCH held rev ${held.revision} vs engine rev ${truth.revision}; objects ${held.objects.length} vs ${truth.objects.length}`);
  }
}
console.log(JSON.stringify({ rounds: ROUNDS, bad, stats }));
process.exit(bad ? 1 : 0);
