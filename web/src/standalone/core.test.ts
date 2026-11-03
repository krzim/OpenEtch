// core.ts's adapter: the shell's /api/_shell routes onto crates/webcore's Backend (a fake package here, recording what
// the wasm would be asked): records kept until boot, then given to the constructor; after boot a record goes to
// /core/store; projects, recovery copies, saves, thumbnails and templates map to /core routes; fonts to add_font;
// `since` and preview jobs pass through.

import { test } from "node:test";
import assert from "node:assert/strict";
import { WasmCore, type BackendPkg, type RawBackend } from "./core.ts";

const enc = new TextEncoder();
const dec = new TextDecoder();

function fakePkg() {
  const calls: { method: string; path: string; query: string; body: string; since?: string }[] = [];
  const inits: unknown[] = [];
  const fonts: string[] = [];
  class Backend implements RawBackend {
    constructor(init: string) {
      inits.push(JSON.parse(init));
    }
    handle(method: string, path: string, query: string, body: Uint8Array, since?: string) {
      calls.push({ method, path, query, body: dec.decode(body), since });
      const status = path === "/core/thumbnail.png" ? 404 : 200;
      return { status, headers: [["content-type", "application/json"]], body: enc.encode(JSON.stringify({ ok: true })), events: [], writes: [] };
    }
    notes() {
      return "[]";
    }
    preview() {
      const a = (n: number) => new Float32Array(n);
      return {
        meta: () => '{"time_s":1}',
        take_xy: () => a(4),
        take_t: () => a(2),
        take_info: () => new Uint32Array(1),
        take_pass: () => new Uint32Array(1),
        take_power: () => a(1),
        take_levels: () => new Uint8Array(1),
        take_row_segments: () => new Uint32Array(0),
        take_row_span: () => a(0),
        free: () => {},
      };
    }
    stats() {
      return '{"time_s":1}';
    }
    fill_region_preview(p: string) {
      return p;
    }
    revision() {
      return 7;
    }
    free() {}
  }
  const pkg: BackendPkg = {
    Backend,
    add_font: (family: string) => {
      fonts.push(family);
      return family === "junk" ? JSON.stringify({ error: "not a TrueType or OpenType font" }) : JSON.stringify({ families: ["OpenEtch Box"] });
    },
  };
  return { pkg, calls, inits, fonts };
}

test("records before boot go to the constructor; after boot to /core/store", () => {
  const f = fakePkg();
  const c = new WasmCore(f.pkg);
  assert.equal(c.handle("PUT", "/api/_shell/store", "store=prefs&key=preferences", enc.encode('{"a":1}')).status, 204);
  c.handle("PUT", "/api/_shell/store", "store=devices&key=x", enc.encode("toml"));
  c.handle("DELETE", "/api/_shell/store", "store=devices&key=x", new Uint8Array());
  assert.equal(f.inits.length, 0);
  assert.equal(c.handle("POST", "/api/_shell/boot", "", new Uint8Array()).status, 200);
  assert.equal(f.inits.length, 1);
  const init = f.inits[0] as { stores: unknown[]; epoch: string };
  assert.deepEqual(init.stores, [{ store: "prefs", key: "preferences", value: '{"a":1}' }]);
  assert.ok(init.epoch.length > 0);
  c.handle("PUT", "/api/_shell/store", "store=prefs&key=preferences", enc.encode('{"a":2}'));
  assert.deepEqual(f.calls.at(-1), { method: "PUT", path: "/core/store", query: "store=prefs&key=preferences", body: '{"a":2}', since: undefined });
});

test("shell routes map onto the core's", () => {
  const f = fakePkg();
  const c = new WasmCore(f.pkg);
  c.handle("POST", "/api/_shell/open_project", "name=Coaster&new_document=1", enc.encode("{}"));
  assert.deepEqual([f.calls.at(-1)?.path, f.calls.at(-1)?.query], ["/core/open", "kind=project&name=Coaster&new_document=1"]);
  c.handle("POST", "/api/_shell/recover", "display=tag.lbrn2", enc.encode("{}"));
  assert.deepEqual([f.calls.at(-1)?.path, f.calls.at(-1)?.query], ["/core/open", "kind=recovery&display=tag.lbrn2"]);
  c.handle("POST", "/api/_shell/project_saved", "", enc.encode(JSON.stringify({ name: "Coaster", revision: 4, document_id: "doc-2" })));
  assert.deepEqual([f.calls.at(-1)?.path, f.calls.at(-1)?.query], ["/core/saved", "name=Coaster&revision=4&document=doc-2"]);
  c.handle("POST", "/api/_shell/thumbnail", "width=64", enc.encode("{}"));
  // Without resvg the PNG route is 404: the SVG one answers.
  assert.deepEqual(f.calls.slice(-2).map((x) => x.path), ["/core/thumbnail.png", "/core/thumbnail.svg"]);
  c.handle("POST", "/api/_shell/check_template", "", enc.encode("<x/>"));
  assert.deepEqual([f.calls.at(-1)?.path, f.calls.at(-1)?.query], ["/core/template/check", "format=lbt"]);
  c.handle("POST", "/api/execute", "", enc.encode("{}"), "e:3");
  assert.equal(f.calls.at(-1)?.since, "e:3");
});

test("fonts go to add_font; a refusal is a 400", () => {
  const f = fakePkg();
  const c = new WasmCore(f.pkg);
  const ok = c.handle("POST", "/api/_shell/font", "name=box.ttf", new Uint8Array([1]));
  assert.equal(ok.status, 200);
  assert.equal(JSON.parse(dec.decode(ok.body)).family, "OpenEtch Box");
  assert.equal(f.fonts[0], "box");
  assert.equal(c.handle("POST", "/api/_shell/font", "name=junk.ttf", new Uint8Array([1])).status, 400);
});

test("preview jobs: packed arrays for full, JSON for stats and fill, with the revision", () => {
  const c = new WasmCore(fakePkg().pkg);
  const full = c.preview("full");
  assert.equal(full.rev, 7);
  assert.equal(full.arrays?.xy.length, 4);
  assert.equal(c.preview("stats").json, '{"time_s":1}');
  assert.equal(c.preview("fill", { points: [[1, 2]] }).json, '{"points":[[1,2]]}');
});
