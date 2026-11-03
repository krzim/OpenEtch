// HTTP calls of the workspace layer: the open documents (LightBurn's New Window as document tabs, ARCHITECTURE.md
// "Open documents") and the LightBurn settings a layer has that will not be executed (`GET /api/layers/unsupported`).
// Same auth and error handling as api/client.ts (bearer token, ApiError with the engine's `error`, `code` and status).
// The answers are parsed into plain types here (pure, so `npm test` covers the parsers).

import { ApiError, authToken, UI_ORIGIN } from "../api/client.ts";
import { transport } from "../api/transport.ts";

async function call<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  const h: Record<string, string> = { Accept: "application/json" };
  if (body !== undefined) h["Content-Type"] = "application/json";
  const token = authToken();
  if (token) h.Authorization = `Bearer ${token}`;
  let res: Response;
  try {
    res = await transport(method, path, { headers: h, body: body === undefined ? undefined : JSON.stringify(body) });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  const text = await res.text();
  let data: unknown = undefined;
  try {
    data = text ? JSON.parse(text) : undefined;
  } catch {
    data = text;
  }
  const obj = data && typeof data === "object" ? (data as { error?: unknown; code?: unknown }) : null;
  if (!res.ok) {
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    const code = obj && typeof obj.code === "string" ? obj.code : undefined;
    throw new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code, detail: msg });
  }
  return data as T;
}

// ---- open documents ---------------------------------------------------------------------------------------------------

export interface OpenDocument {
  id: string;
  /** What the tab shows: the project's name, else the document's. */
  title: string;
  project: string | null;
  dirty: boolean;
  active: boolean;
  objects: number;
}

export interface DocumentList {
  active: string | null;
  documents: OpenDocument[];
}

const str = (v: unknown): string | null => (typeof v === "string" && v.length > 0 ? v : null);

/** `GET /api/documents` → tabs; null when the answer is not a document list (an older engine). */
export function parseDocuments(raw: unknown): DocumentList | null {
  if (!raw || typeof raw !== "object" || !Array.isArray((raw as { documents?: unknown }).documents)) return null;
  const r = raw as { active?: unknown; documents: unknown[] };
  const active = str(r.active);
  const documents: OpenDocument[] = [];
  for (const d of r.documents) {
    if (!d || typeof d !== "object") continue;
    const o = d as Record<string, unknown>;
    const id = str(o.id);
    if (!id) continue;
    const project = str(o.project);
    documents.push({
      id,
      title: str(o.display_name) ?? project ?? str(o.document_name) ?? str(o.name) ?? "Untitled",
      project,
      dirty: o.dirty === true,
      active: o.active === true || id === active,
      objects: typeof o.objects === "number" ? o.objects : 0,
    });
  }
  return { active: active ?? documents.find((d) => d.active)?.id ?? null, documents };
}

/** The tab to show after closing `closing`: the engine activates the one opened before it; this is the guess until it says. */
export function neighbourAfterClose(list: DocumentList, closing: string): string | null {
  const i = list.documents.findIndex((d) => d.id === closing);
  if (i < 0) return list.active;
  const rest = list.documents.filter((d) => d.id !== closing);
  return rest[Math.max(0, i - 1)]?.id ?? null;
}

export const wsApi = {
  documents: async () => parseDocuments(await call("GET", "/api/documents")),
  /** A new document (`project`: open that saved project into it). */
  openDocument: (project?: string) => call<{ id?: string; existing?: boolean }>("POST", "/api/documents", project ? { project, origin: UI_ORIGIN } : { origin: UI_ORIGIN }),
  activate: (id: string) => call("POST", `/api/documents/${encodeURIComponent(id)}/activate`, { origin: UI_ORIGIN }),
  /** 409 with unsaved changes unless `discard`. */
  close: (id: string, discard: boolean) => call("POST", `/api/documents/${encodeURIComponent(id)}/close`, discard ? { discard: true } : {}),
  unsupported: async () => parseUnsupported(await call("GET", "/api/layers/unsupported")),
};

// ---- LightBurn settings that will not be executed (LAY-034) -------------------------------------------------------------

export interface UnsupportedSetting {
  setting: string;
  value: string;
  /** null: the main settings, else the 0-based sub-layer. */
  subLayer: number | null;
  /** true: on in the file and not done by OpenEtch; null: an unknown setting whose effect is not known. */
  active: boolean | null;
  note: string;
}

/** `GET /api/layers/unsupported` → layer id → its settings (only layers that have some). */
export function parseUnsupported(raw: unknown): Map<string, UnsupportedSetting[]> {
  const out = new Map<string, UnsupportedSetting[]>();
  const layers = raw && typeof raw === "object" ? (raw as { layers?: unknown }).layers : null;
  if (!Array.isArray(layers)) return out;
  for (const l of layers) {
    if (!l || typeof l !== "object") continue;
    const o = l as { layer_id?: unknown; unsupported?: unknown };
    const id = str(o.layer_id);
    if (!id || !Array.isArray(o.unsupported)) continue;
    const list: UnsupportedSetting[] = [];
    for (const u of o.unsupported) {
      if (!u || typeof u !== "object") continue;
      const x = u as Record<string, unknown>;
      list.push({
        setting: str(x.setting) ?? str(x.tag) ?? "?",
        value: typeof x.value === "string" ? x.value : String(x.value ?? ""),
        subLayer: typeof x.sub_layer === "number" ? x.sub_layer : null,
        active: x.active === true ? true : x.active === false ? false : null,
        note: typeof x.note === "string" ? x.note : "",
      });
    }
    if (list.length) out.set(id, list);
  }
  return out;
}

/** Whether a document has LightBurn settings worth asking about (only imported layers carry `lbrn2.*` extras). */
export function hasLightBurnExtras(layers: readonly { extras?: Record<string, string> }[]): boolean {
  return layers.some((l) => l.extras && Object.keys(l.extras).some((k) => k.startsWith("lbrn2.")));
}

/** The badge's tooltip: one line per setting, "Wobble (sub-layer 2) = 1: <the engine's note>". */
export function unsupportedText(list: readonly UnsupportedSetting[], subLayer: (n: number) => string): string {
  return list
    .filter((u) => u.active !== false)
    .map((u) => `${u.setting}${u.subLayer !== null ? ` (${subLayer(u.subLayer + 1)})` : ""}${u.value ? ` = ${u.value}` : ""}${u.note ? `: ${u.note}` : ""}`)
    .join("\n");
}
