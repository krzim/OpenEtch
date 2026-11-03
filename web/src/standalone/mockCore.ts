// A stand-in for crates/webcore's backend (contract.ts `WebcoreBackend`), used by the shell's tests and, until the
// core track lands, by the worker (VITE_STANDALONE_MOCK=1). It keeps documents with a name, a revision and a list of
// objects, the engine's project state (name, saved / clean revision, display name), a few stores webcore will own
// (prefs) and the internal `/api/_shell/...` routes, with the engine's JSON shapes. It is not the engine: commands
// only add objects or bump the revision.
//
// `POST /api/_mock/panic` throws like a Rust panic does in wasm, and every call after it throws too (a poisoned
// instance), so the crash and restart path can be tested.

import type { CoreReply, CoreWrite, WebcoreBackend } from "./contract.ts";
import { bytes, text } from "./reply.ts";

interface MockDoc {
  id: string;
  name: string;
  revision: number;
  objects: { id: string; kind: string }[];
  /** Project state (crates/engine/src/projects.rs `ProjectState`). */
  project: { name: string | null; saved_revision: number | null; clean_revision: number | null; display_name: string | null };
}

/** A 1×1 PNG, the mock's thumbnail. */
const PNG_1X1 = Uint8Array.from(
  atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="),
  (c) => c.charCodeAt(0),
);

export class MockCore implements WebcoreBackend {
  private docs: MockDoc[] = [];
  private active = "";
  private nextDoc = 1;
  private nextObj = 1;
  /** A revision counter shared by every document (the engine's high-water mark: revisions never repeat). */
  private highWater = 0;
  private poisoned = false;
  /** Stores loaded through `PUT /api/_shell/store` and changed by `PUT /api/prefs`. */
  readonly stores = new Map<string, Map<string, Uint8Array>>();
  readonly fonts: string[] = [];
  booted: { now_unix_ms: number; tz_offset_min: number } | null = null;
  /** Every request, for tests. */
  readonly log: string[] = [];
  /** Tests: a path that panics like `/api/_mock/panic`. */
  panicOn: string | null = null;

  constructor() {
    this.active = this.newDoc().id;
  }

  handle(method: string, path: string, query: string, body: Uint8Array): CoreReply {
    if (this.poisoned) throw new Error("RuntimeError: unreachable (the instance panicked before)");
    this.log.push(`${method} ${path}${query ? `?${query}` : ""}`);
    if (path === this.panicOn) {
      this.poisoned = true;
      throw new Error("RuntimeError: unreachable");
    }
    const out: CoreReply = { status: 200, headers: [["content-type", "application/json"]], body: new Uint8Array(), events: [], writes: [] };
    const q = new URLSearchParams(query);
    const ok = (v: unknown, status = 200) => {
      out.status = status;
      out.body = bytes(JSON.stringify(v));
      return out;
    };
    const fail = (status: number, error: string, code?: string) => ok(code ? { ok: false, error, code } : { ok: false, error }, status);
    const doc = this.doc();
    const key = `${method} ${path}`;
    switch (key) {
      case "POST /api/_mock/panic":
        this.poisoned = true;
        throw new Error("RuntimeError: unreachable");
      case "GET /api/project":
        return ok(this.projectStatus(doc));
      case "GET /api/documents":
        return ok({
          active: this.active,
          documents: this.docs.map((d) => ({
            id: d.id,
            name: d.project.name ?? d.project.display_name ?? d.name,
            project: d.project.name,
            display_name: d.project.display_name,
            document_name: d.name,
            revision: d.revision,
            dirty: dirty(d),
            objects: d.objects.length,
            active: d.id === this.active,
          })),
        });
      case "POST /api/documents": {
        const d = this.newDoc();
        this.active = d.id;
        out.events.push(JSON.stringify({ type: "documents_changed", active: d.id }));
        return ok({ ok: true, id: d.id, active: d.id, existing: false, revision: d.revision });
      }
      case "GET /api/document":
        return ok({ name: doc.name, revision: doc.revision, objects: doc.objects });
      case "POST /api/execute": {
        const inv = JSON.parse(text(body)) as { id: string; params?: Record<string, unknown> };
        if (inv.id === "doc.new") {
          doc.objects = [];
          doc.name = "Untitled";
          this.bump(doc, out, true);
          doc.project = { name: null, saved_revision: null, clean_revision: doc.revision, display_name: null };
        } else if (inv.id === "doc.rename") {
          doc.name = String(inv.params?.name ?? doc.name);
          this.bump(doc, out);
        } else if (inv.id.startsWith("shape.")) {
          doc.objects.push({ id: `o${this.nextObj++}`, kind: inv.id.slice(6) });
          this.bump(doc, out);
        } else {
          this.bump(doc, out);
        }
        return ok({ ok: true, result: {}, revision: doc.revision });
      }
      case "GET /api/document/download": {
        if ((q.get("format") ?? "openetch") !== "openetch") return fail(400, "the mock core downloads only .openetch");
        const stem = doc.project.name ?? doc.project.display_name ?? doc.name;
        out.headers = [
          ["content-type", "application/json; charset=utf-8"],
          ["content-disposition", `attachment; filename="${stem.split("/").pop()}.openetch"`],
          ["x-openetch-warnings", "0"],
        ];
        out.body = bytes(serialise(doc));
        return out;
      }
      case "POST /api/document/upload": {
        const format = q.get("format");
        if (body.length === 0) return fail(400, "the upload is empty");
        const loaded = format === "openetch" ? parse(text(body)) : format === "lbrn2" && text(body).includes("<LightBurnProject") ? { name: "Imported", objects: [] } : null;
        if (!loaded) return ok({ ok: false, error: `doc.load: not an OpenEtch file` });
        this.load(doc, loaded, out);
        const name = q.get("name");
        doc.project = { name: null, saved_revision: null, clean_revision: doc.revision, display_name: name ? name.replace(/\.(openetch|lbrn2|lbt)$/i, "") : null };
        return ok({ ok: true, result: { objects: doc.objects.length }, revision: doc.revision });
      }
      case "POST /api/_shell/open_project": {
        const name = q.get("name") ?? "";
        const loaded = parse(text(body));
        if (!loaded) return fail(400, `project "${name}": not an OpenEtch file`);
        let target = doc;
        if (q.get("new_document") === "1") {
          const holding = this.docs.find((d) => d.project.name?.toLowerCase() === name.toLowerCase());
          if (holding) {
            this.active = holding.id;
            out.events.push(JSON.stringify({ type: "documents_changed", active: holding.id }));
            return ok({ ok: true, id: holding.id, active: holding.id, existing: true, revision: holding.revision });
          }
          target = this.newDoc();
          this.active = target.id;
          out.events.push(JSON.stringify({ type: "documents_changed", active: target.id }));
        }
        this.load(target, loaded, out);
        target.project = { name, saved_revision: target.revision, clean_revision: null, display_name: null };
        const result = { ok: true, name, result: { objects: target.objects.length }, revision: target.revision };
        return q.get("new_document") === "1" ? ok({ ok: true, id: target.id, active: target.id, existing: false, revision: target.revision, result }) : ok(result);
      }
      case "POST /api/_shell/recover": {
        const loaded = parse(text(body));
        if (!loaded) return fail(400, "not an OpenEtch file");
        this.load(doc, loaded, out);
        const project = q.get("project");
        doc.project = { name: project, saved_revision: null, clean_revision: null, display_name: project ? null : q.get("display") };
        return ok({ ok: true, result: { objects: doc.objects.length }, revision: doc.revision });
      }
      case "POST /api/_shell/project_saved": {
        const b = JSON.parse(text(body)) as { name: string; revision: number; document_id: string };
        const d = this.docs.find((x) => x.id === b.document_id);
        if (d) d.project = { name: b.name, saved_revision: b.revision, clean_revision: null, display_name: null };
        out.events.push(JSON.stringify({ type: "project_saved", name: b.name, revision: b.revision, document_id: b.document_id }));
        return ok({ ok: true });
      }
      case "POST /api/_shell/thumbnail": {
        if (!parse(text(body))) return fail(400, "not an OpenEtch file");
        out.headers = [["content-type", "image/png"]];
        out.body = PNG_1X1.slice();
        return out;
      }
      case "POST /api/_shell/check_template":
        return text(body).includes("<LightBurnProject") ? ok({ ok: true }) : fail(400, "not a LightBurn template: no LightBurnProject element");
      case "PUT /api/_shell/store":
      case "DELETE /api/_shell/store": {
        const store = q.get("store") ?? "";
        const k = q.get("key") ?? "";
        let m = this.stores.get(store);
        if (!m) this.stores.set(store, (m = new Map()));
        if (method === "PUT") m.set(k, body.slice());
        else m.delete(k);
        out.status = 204;
        if (store === "libraries") out.events.push(JSON.stringify({ type: "art_library_changed" }));
        return out;
      }
      case "POST /api/_shell/boot":
        this.booted = JSON.parse(text(body)) as { now_unix_ms: number; tz_offset_min: number };
        return ok({ ok: true });
      case "POST /api/_shell/font": {
        const name = q.get("name") ?? "";
        const magic = text(body.slice(0, 4));
        if (body.length < 12 || !(magic === "OTTO" || magic === "true" || magic === "ttcf" || (body[0] === 0 && body[1] === 1 && body[2] === 0 && body[3] === 0))) {
          return fail(400, `${name}: not a TrueType or OpenType font`);
        }
        const family = name.replace(/\.(ttf|otf|ttc)$/i, "");
        this.fonts.push(family);
        return ok({ ok: true, family });
      }
      case "GET /api/prefs": {
        const v = this.stores.get("prefs")?.get("prefs");
        return ok(v ? JSON.parse(text(v)) : { units: "mm" });
      }
      case "PUT /api/prefs": {
        const v = text(body);
        let m = this.stores.get("prefs");
        if (!m) this.stores.set("prefs", (m = new Map()));
        m.set("prefs", bytes(v));
        const w: CoreWrite = { store: "prefs", key: "prefs", value: v };
        out.writes.push(w);
        return ok(JSON.parse(v));
      }
      case "GET /api/machine":
        return ok({ connected: false, design_only: true, runs: [], pending_runs: [] });
      case "GET /api/job":
        return ok({ state: "idle" });
      default:
        return fail(404, "not available in the design-only build", "design_only");
    }
  }

  private doc(): MockDoc {
    return this.docs.find((d) => d.id === this.active) ?? this.docs[0];
  }

  private newDoc(): MockDoc {
    const id = this.docs.length === 0 ? "default" : `doc-${++this.nextDoc}`;
    const d: MockDoc = { id, name: "Untitled", revision: ++this.highWater, objects: [], project: { name: null, saved_revision: null, clean_revision: null, display_name: null } };
    d.project.clean_revision = d.revision;
    this.docs.push(d);
    return d;
  }

  private bump(d: MockDoc, out: CoreReply, history = false): void {
    d.revision = ++this.highWater;
    out.events.push(JSON.stringify({ type: "document_changed", revision: d.revision, origin: "ui", document_id: d.id }));
    if (history) out.events.push(JSON.stringify({ type: "history_changed", revision: d.revision }));
  }

  private load(d: MockDoc, loaded: { name: string; objects: { id: string; kind: string }[] }, out: CoreReply): void {
    d.name = loaded.name;
    d.objects = loaded.objects;
    this.bump(d, out);
  }

  private projectStatus(d: MockDoc) {
    return { name: d.project.name, display_name: d.project.display_name, saved_revision: d.project.saved_revision, revision: d.revision, dirty: dirty(d) };
  }

  /** Test helper: the active document's id. */
  activeId(): string {
    return this.active;
  }
}

function dirty(d: MockDoc): boolean {
  return d.project.saved_revision !== d.revision && d.project.clean_revision !== d.revision;
}

function serialise(d: MockDoc): string {
  return JSON.stringify({ format: "openetch", version: 1, name: d.name, objects: d.objects });
}

function parse(t: string): { name: string; objects: { id: string; kind: string }[] } | null {
  try {
    const v = JSON.parse(t) as { format?: unknown; name?: unknown; objects?: unknown };
    if (v.format !== "openetch" || !Array.isArray(v.objects)) return null;
    return { name: typeof v.name === "string" ? v.name : "Untitled", objects: v.objects as { id: string; kind: string }[] };
  } catch {
    return null;
  }
}

/** A minimal `.openetch` file for tests. */
export function mockProjectFile(name: string, objects = 1): Uint8Array {
  return bytes(JSON.stringify({ format: "openetch", version: 1, name, objects: Array.from({ length: objects }, (_, i) => ({ id: `o${i + 1}`, kind: "rect" })) }));
}

