// Unit tests for api/project.ts (project names, file names, import/export answers, warnings, dirty state) and the
// File routes in api/client.ts against a fake fetch: upload sends the raw file with the bearer header, download
// reads the Content-Disposition name, save/open check `{ok:false}`.

import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

interface Call {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
}

const calls: Call[] = [];
let reply: { status: number; body: string; headers?: Record<string, string> } = { status: 200, body: "{}" };
const store = new Map<string, string>([["openetch.token", "tok-9"]]);

const g = globalThis as Record<string, unknown>;
g.localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => void store.set(k, v),
};
g.window = { location: { protocol: "http:", host: "pi.local:7878", href: "http://pi.local:7878/" } };
g.fetch = async (path: string, init: { method: string; headers: Record<string, string>; body?: unknown }) => {
  calls.push({ method: init.method, path, headers: init.headers, body: init.body });
  const headers = new Map(Object.entries(reply.headers ?? {}).map(([k, v]) => [k.toLowerCase(), v]));
  return {
    ok: reply.status >= 200 && reply.status < 300,
    status: reply.status,
    statusText: "",
    headers: { get: (k: string) => headers.get(k.toLowerCase()) ?? null },
    text: async () => reply.body,
    blob: async () => new Blob([reply.body]),
  };
};

const { api, ApiError } = await import("./client.ts");
const P = await import("./project.ts");

beforeEach(() => {
  calls.length = 0;
  reply = { status: 200, body: "{}" };
});

test("project names follow the engine's rule", () => {
  for (const ok of ["Namecards", "Brass tags (v2)", "a-b_c.d", "Ärmel 3", "x".repeat(100)]) {
    assert.equal(P.validateProjectName(ok), null, ok);
  }
  assert.match(P.validateProjectName("") ?? "", /Enter/);
  assert.match(P.validateProjectName("x".repeat(101)) ?? "", /100/);
  for (const bad of ["a/b", "a\\b", "c:x", "tag*", "new\nline"]) assert.match(P.validateProjectName(bad) ?? "", /letters/, bad);
  assert.match(P.validateProjectName(".hidden") ?? "", /dot/);
  assert.match(P.validateProjectName("a..b") ?? "", /\.\./);
});

test("suggested names are valid or empty", () => {
  assert.equal(P.suggestProjectName("Captain America"), "Captain America");
  assert.equal(P.suggestProjectName("photo on namecard!"), "photo on namecard");
  assert.equal(P.suggestProjectName("..secret..file"), "secret.file");
  assert.equal(P.suggestProjectName("a/b:c"), "a b c");
  assert.equal(P.suggestProjectName("***"), "");
  assert.equal([...P.suggestProjectName("y".repeat(300))].length, 100);
  for (const s of ["Untitled", "..", " . . ", "x/../y"]) {
    const n = P.suggestProjectName(s);
    assert.ok(n === "" || P.validateProjectName(n) === null, s);
  }
});

test("file formats, stems and download names", () => {
  assert.equal(P.fileFormat("Job.OPENETCH"), "openetch");
  assert.equal(P.fileFormat("Color_Test.lbrn2"), "lbrn2");
  assert.equal(P.fileFormat("old.lbrn"), null);
  assert.equal(P.fileFormat("art.svg"), null);
  assert.equal(P.fileStem("C:\\x\\Color Test.lbrn2"), "Color Test");
  assert.equal(P.fileStem(".openetch"), ".openetch");
  assert.equal(P.downloadName("Brass tags", "lbrn2"), "Brass tags.lbrn2");
  assert.equal(P.downloadName('a/b:c*"d"', "openetch"), "a_b_c_d_.openetch");
  assert.equal(P.downloadName("", "openetch"), "Untitled.openetch");
  assert.equal(P.downloadName(null, "lbrn2"), "Untitled.lbrn2");
  assert.equal(P.downloadName("..", "lbrn2"), "Untitled.lbrn2");
});

test("Content-Disposition names", () => {
  assert.equal(P.contentDispositionName('attachment; filename="Brass tags.openetch"'), "Brass tags.openetch");
  assert.equal(P.contentDispositionName("attachment; filename=job.lbrn2"), "job.lbrn2");
  assert.equal(P.contentDispositionName(`attachment; filename="x.openetch"; filename*=UTF-8''%C3%84rmel.openetch`), "Ärmel.openetch");
  assert.equal(P.contentDispositionName(`attachment; filename*=UTF-8''%E0%A4%A.openetch; filename="ok.openetch"`), "ok.openetch");
  assert.equal(P.contentDispositionName('attachment; filename="say \\"hi\\".lbrn2"'), 'say "hi".lbrn2');
  assert.equal(P.contentDispositionName("attachment"), null);
  assert.equal(P.contentDispositionName(null), null);
});

test("command answers unwrap and refusals throw the engine's message", () => {
  assert.deepEqual(P.commandResult({ ok: true, result: { created: ["obj-1"] }, revision: 3 }), { created: ["obj-1"] });
  assert.deepEqual(P.commandResult({ created: [] }), { created: [] });
  assert.throws(() => P.commandResult({ ok: false, error: "doc.import_lbrn2: not a LightBurn project" }), /not a LightBurn project/);
  assert.throws(() => P.commandResult({ ok: false }), /refused/);
});

test("import outcome, message and grouped warnings", () => {
  const text = "text was imported as its outline (BackupPath)";
  const r = {
    created: ["obj-1", "obj-2", "obj-3"],
    created_layers: ["layer-1", "layer-2"],
    reused_layers: [],
    warnings: ["C12: no frequency in the file; using 30 kHz", text, text, 7, ""],
    bounds: null,
    mode: "replace",
    field_mm: 150,
  };
  const o = P.importOutcome(r);
  assert.deepEqual(o, { objects: 3, createdLayers: 2, reusedLayers: 0, warnings: [r.warnings[0], text, text], fieldMm: 150 });
  assert.equal(P.importMessage(o, "replace"), "Imported 3 objects on 2 layers (field 150 mm). 3 warnings.");
  const merged = P.importOutcome({ created: ["obj-9"], created_layers: [], reused_layers: ["layer-1"], warnings: [] });
  assert.equal(P.importMessage(merged, "merge"), "Merged 1 object on 0 new layers, 1 reused.");
  assert.deepEqual(P.groupWarnings(o.warnings), [
    { text: r.warnings[0], count: 1 },
    { text, count: 2 },
  ]);
  assert.deepEqual(P.importOutcome(null), { objects: 0, createdLayers: 0, reusedLayers: 0, warnings: [], fieldMm: null });
  // doc.load reports its layers as `layers`.
  assert.equal(P.importOutcome({ created: [], layers: ["layer-1"] }).createdLayers, 1);
});

test("project list: array or wrapped, valid rows, newest first", () => {
  const rows = [
    { name: "old", modified_unix_s: 100, size: 10 },
    { name: "new", modified_unix_s: 300, size: 20 },
    { nope: true },
    { name: "b", modified_unix_s: 200 },
    { name: "a", modified_unix_s: 200 },
  ];
  assert.deepEqual(P.projectList(rows).map((p) => p.name), ["new", "a", "b", "old"]);
  assert.deepEqual(P.projectList({ projects: rows.slice(0, 2) }).map((p) => p.name), ["new", "old"]);
  assert.deepEqual(P.projectList({ ok: false }), []);
  assert.deepEqual(P.projectList(null), []);
});

test("dirty state follows the document revision", () => {
  const saved = { name: "Tags", saved_revision: 5, revision: 5, dirty: false };
  assert.equal(P.projectDirty(saved, 5), false);
  // An edit landed before /api/project was refetched.
  assert.equal(P.projectDirty(saved, 6), true);
  assert.equal(P.projectDirty({ ...saved, revision: 6, dirty: true }, 5), false);
  assert.equal(P.projectDirty({ name: null, saved_revision: null, revision: 2, dirty: true }, 2), true);
  assert.equal(P.projectDirty({ name: null, saved_revision: null, revision: 2, dirty: false }, 3), true);
  assert.equal(P.projectDirty(null, 3), null);
});

test("a replaced document is not the project the engine still names", () => {
  const p = { name: "Tags", saved_revision: 5, revision: 6, dirty: true };
  assert.equal(P.currentProjectName(p, undefined), "Tags");
  assert.equal(P.currentProjectName(p, "Tags"), null);
  assert.equal(P.currentProjectName({ ...p, name: "Other" }, "Tags"), "Other");
  assert.equal(P.currentProjectName({ ...p, name: null }, null), null);
  assert.equal(P.currentProjectName(null, undefined), null);
});

test("confirmation before replacing", () => {
  const empty = { revision: 1, name: "Untitled", stock: null, layers: [], objects: [], next_id: 1 };
  assert.equal(P.needsConfirm(true, empty), true);
  assert.equal(P.needsConfirm(false, null), false);
  // No project state from the engine: ask whenever there is something to lose.
  assert.equal(P.needsConfirm(null, empty), false);
  assert.equal(P.needsConfirm(null, { ...empty, objects: [{ id: "obj-1" }] } as never), true);
});

test("field sizes and list formatting", () => {
  assert.deepEqual(P.fieldChoices(150, 153.75), [150, 153.75, 110]);
  assert.deepEqual(P.fieldChoices(null, null), [110, 150]);
  assert.equal(P.validField(0), null);
  assert.equal(P.validField(Number.NaN), null);
  assert.equal(P.validField("150"), null);
  assert.equal(P.validField(153.75), 153.75);
  assert.equal(P.sameField(150, 150.0000001), true);
  assert.equal(P.sameField(150, 153.75), false);
  assert.equal(P.sameField(150, null), false);
  assert.equal(P.formatBytes(512), "512 B");
  assert.equal(P.formatBytes(2048), "2.0 KB");
  assert.equal(P.formatBytes(5 * 1024 * 1024), "5.0 MB");
  assert.equal(P.formatBytes(undefined), "");
  const now = 1_000_000 * 1000;
  assert.equal(P.formatModified(1_000_000 - 30, now), "just now");
  assert.equal(P.formatModified(1_000_000 - 600, now), "10 min ago");
  assert.equal(P.formatModified(1_000_000 - 7200, now), "2 h ago");
  assert.notEqual(P.formatModified(1_000_000 - 7 * 86400, now), "");
  assert.equal(P.formatModified(undefined, now), "");
});

test("upload sends the raw file with the bearer header to the contract route", async () => {
  reply = { status: 200, body: JSON.stringify({ ok: true, result: { created: [] }, revision: 2 }) };
  const file = new Blob(["<LightBurnProject/>"]);
  const res = await api.upload(file, "lbrn2", "merge");
  assert.equal(calls[0]?.method, "POST");
  assert.equal(calls[0]?.path, "/api/document/upload?format=lbrn2&mode=merge");
  assert.equal(calls[0]?.headers.Authorization, "Bearer tok-9");
  assert.equal(calls[0]?.headers["Content-Type"], "application/octet-stream");
  assert.equal(calls[0]?.body, file);
  assert.deepEqual(P.commandResult(res), { created: [] });

  reply = { status: 413, body: JSON.stringify({ ok: false, error: "file is larger than 50 MB" }) };
  await assert.rejects(api.upload(file, "openetch"), (e: unknown) => {
    assert.ok(e instanceof ApiError);
    assert.equal(e.status, 413);
    assert.equal(e.detail, "file is larger than 50 MB");
    return true;
  });
  assert.equal(calls[1]?.path, "/api/document/upload?format=openetch&mode=replace");

  // A LightBurn import at another field size passes it on the same route; .openetch never sends one.
  reply = { status: 200, body: JSON.stringify({ ok: true, result: { created: [] }, revision: 3 }) };
  await api.upload(file, "lbrn2", "replace", 110);
  assert.equal(calls[2]?.path, "/api/document/upload?format=lbrn2&mode=replace&field_mm=110");
  await api.upload(file, "openetch", "replace", 110);
  assert.equal(calls[3]?.path, "/api/document/upload?format=openetch&mode=replace");
});

test("download fetches with the bearer header and reads the file name", async () => {
  reply = { status: 200, body: '{"format":"openetch"}', headers: { "Content-Disposition": 'attachment; filename="Tags.openetch"' } };
  const { blob, filename } = await api.download("openetch");
  assert.equal(calls[0]?.path, "/api/document/download?format=openetch");
  assert.equal(calls[0]?.headers.Authorization, "Bearer tok-9");
  assert.equal(filename, "Tags.openetch");
  assert.equal(await blob.text(), '{"format":"openetch"}');

  reply = { status: 200, body: "<x/>" };
  assert.equal((await api.download("lbrn2")).filename, null);

  reply = { status: 422, body: JSON.stringify({ ok: false, error: "more than 30 layers" }) };
  await assert.rejects(api.download("lbrn2"), /more than 30 layers/);
});

test("project routes: list, save, open, state", async () => {
  reply = { status: 200, body: JSON.stringify([{ name: "Tags", modified_unix_s: 1, size: 3 }]) };
  assert.deepEqual(P.projectList(await api.projects()), [{ name: "Tags", modified_unix_s: 1, size: 3 }]);
  reply = { status: 200, body: JSON.stringify({ ok: true, name: "Tags", path: "/p/Tags.openetch" }) };
  await api.saveProject("Tags");
  await api.openProject("Tags");
  reply = { status: 200, body: JSON.stringify({ name: "Tags", saved_revision: 4, revision: 4, dirty: false }) };
  assert.equal((await api.project()).dirty, false);
  assert.deepEqual(
    calls.map((c) => `${c.method} ${c.path} ${c.body === undefined ? "" : String(c.body)}`),
    ["GET /api/projects ", 'POST /api/projects/save {"name":"Tags"}', 'POST /api/projects/open {"name":"Tags"}', "GET /api/project "],
  );
  for (const c of calls) assert.equal(c.headers.Authorization, "Bearer tok-9");

  reply = { status: 200, body: JSON.stringify({ ok: false, error: "invalid project name" }) };
  await assert.rejects(api.saveProject("a/b"), /invalid project name/);
  reply = { status: 404, body: JSON.stringify({ ok: false, error: "no project named \"Nope\"" }) };
  await assert.rejects(api.openProject("Nope"), (e: unknown) => e instanceof ApiError && e.status === 404);
});
