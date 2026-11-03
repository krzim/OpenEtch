// HTTP calls of the job ledger (crates/engine/src/api_ledger.rs): search, one entry, its thumbnail, verdict and
// note, Re-run / Open as burned, the storage settings. Same auth and errors as api/client.ts (bearer token, ApiError
// with the engine's text). Nothing here starts a burn: Re-run asks the engine for a *pending* run, which a person
// starts in the Laser window like any other.

import { ApiError, authToken, UI_ORIGIN } from "../api/client.ts";
import { ledgerQuery, settingsBody, type LedgerFilter, type LedgerSettings, type Verdict } from "./ledgerModel.ts";
import { transport } from "../api/transport.ts";

function headers(accept: string, json: boolean): Record<string, string> {
  const h: Record<string, string> = { Accept: accept };
  if (json) h["Content-Type"] = "application/json";
  const token = authToken();
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

async function fetchOk(method: string, path: string, body?: unknown, accept = "application/json"): Promise<Response> {
  let res: Response;
  try {
    res = await transport(method, path, { headers: headers(accept, body !== undefined), body: body === undefined ? undefined : JSON.stringify(body) });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (!res.ok) {
    const text = await res.text();
    let msg = text || res.statusText;
    let code: string | undefined;
    try {
      const d = JSON.parse(text) as { error?: unknown; code?: unknown };
      if (typeof d.error === "string") msg = d.error;
      if (typeof d.code === "string") code = d.code;
    } catch {
      // not JSON
    }
    throw new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code, detail: msg });
  }
  return res;
}

async function json(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetchOk(method, path, body);
  const text = await res.text();
  return text ? (JSON.parse(text) as unknown) : undefined;
}

export const ledgerApi = {
  list: (f: LedgerFilter) => json("GET", `/api/job-log?${ledgerQuery(f)}`),
  csv: async (f: LedgerFilter): Promise<Blob> => (await fetchOk("GET", `/api/job-log?${ledgerQuery(f, "csv")}`, undefined, "text/csv")).blob(),
  thumbnail: async (id: string): Promise<Blob> => (await fetchOk("GET", `/api/job-log/${encodeURIComponent(id)}/thumbnail.png`, undefined, "image/png")).blob(),
  mark: (id: string, verdict: Verdict | null, note: string) => json("POST", `/api/job-log/${encodeURIComponent(id)}/mark`, { verdict, note, origin: UI_ORIGIN }),
  open: (id: string) => json("POST", `/api/job-log/${encodeURIComponent(id)}/open`, { origin: UI_ORIGIN }),
  rerun: (id: string) => json("POST", `/api/job-log/${encodeURIComponent(id)}/rerun`, { origin: UI_ORIGIN }),
  settings: () => json("GET", "/api/job-ledger/settings"),
  saveSettings: (s: LedgerSettings) => json("PUT", "/api/job-ledger/settings", settingsBody(s)),
};

// ---- thumbnails: fetched with the token (an <img src> cannot carry it), kept as object URLs ----------------------

const MAX_CACHED = 400;
const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string | null>>();

/** An object URL of entry `id`'s thumbnail (cached; null when the engine has none). */
export function thumbnailUrl(id: string): Promise<string | null> {
  const hit = cache.get(id);
  if (hit) return Promise.resolve(hit);
  const running = inflight.get(id);
  if (running) return running;
  const p = ledgerApi
    .thumbnail(id)
    .then((b) => {
      const url = URL.createObjectURL(b);
      cache.set(id, url);
      while (cache.size > MAX_CACHED) {
        const [old, u] = cache.entries().next().value as [string, string];
        cache.delete(old);
        URL.revokeObjectURL(u);
      }
      return url;
    })
    .catch(() => null)
    .finally(() => inflight.delete(id));
  inflight.set(id, p);
  return p;
}
