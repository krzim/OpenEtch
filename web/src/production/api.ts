// HTTP calls of the production features (engine `production::api`): the cockpit's state, its design lock, Continue at a
// layer pause, the serial ledger and re-marks, the hour meter and reminders, the extractor and the notification targets.
// Every call that changes something sends `origin: "ui"`; the engine refuses them from agents. Nothing here can start
// a burn: a re-mark is a pending run, approved like any other.

import { ApiError, authToken, UI_ORIGIN } from "../api/client.ts";
import type { ExtractorConfig, ProductionStatus, ReminderDraft, TargetDraft } from "./model.ts";
import { transport } from "../api/transport.ts";

function headers(contentType?: string): Record<string, string> {
  const h: Record<string, string> = { Accept: "application/json" };
  if (contentType) h["Content-Type"] = contentType;
  const token = authToken();
  if (token) h.Authorization = `Bearer ${token}`;
  return h;
}

async function call<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await transport(method, path, { headers: headers(body === undefined ? undefined : "application/json"), body: body === undefined ? undefined : JSON.stringify(body) });
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
  const obj = data && typeof data === "object" ? (data as { ok?: unknown; error?: unknown; code?: unknown }) : null;
  if (!res.ok || obj?.ok === false) {
    const msg = obj && "error" in obj ? String(obj.error) : text || res.statusText;
    throw new ApiError(`${method} ${path}: ${res.status} ${msg}`, res.status, { code: typeof obj?.code === "string" ? obj.code : undefined, detail: msg });
  }
  return data as T;
}

async function download(path: string): Promise<{ blob: Blob; filename: string | null }> {
  let res: Response;
  try {
    res = await transport("GET", path, { headers: { ...headers(), Accept: "*/*" } });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (!res.ok) throw new ApiError(`GET ${path}: ${res.status}`, res.status, { detail: await res.text() });
  return { blob: await res.blob(), filename: "OpenEtchSerialLedger.csv" };
}

const enc = encodeURIComponent;

export const prodApi = {
  status: () => call<ProductionStatus>("GET", "/api/production"),
  /** Take or renew (on) or release the cockpit's design lock. */
  lock: (on: boolean, holder: string, label: string, force = false) => call("POST", "/api/production/lock", { on, holder, label, force, origin: UI_ORIGIN }),
  /** Continue a run waiting at a layer (the person's press; the foot switch does the same). */
  continueRun: (runId?: string | null) => call("POST", "/api/job/continue", { ...(runId ? { run_id: runId } : {}), origin: UI_ORIGIN }),
  ledger: (limit: number, run?: string) => call<{ entries: unknown[] }>("GET", `/api/production/ledger?limit=${limit}${run ? `&run=${enc(run)}` : ""}`),
  ledgerCsv: (limit: number) => download(`/api/production/ledger?limit=${limit}&format=csv`),
  /** A pending run re-marking `slot` of ledger run `runId` with its recorded value. */
  remark: (runId: string, timeUnixS: number, slot: number, revision: number | null) =>
    call<{ id: string; shown: string[] }>("POST", "/api/production/remark", {
      run_id: runId,
      time_unix_s: timeUnixS,
      slot,
      ...(revision === null ? {} : { revision }),
      origin: UI_ORIGIN,
    }),
  meter: () => call("GET", "/api/production/meter"),
  setReminders: (reminders: ReminderDraft[]) => call("PUT", "/api/production/maintenance", { reminders, origin: UI_ORIGIN }),
  reminderDone: (id: string) => call("POST", `/api/production/maintenance/${enc(id)}/done`, { origin: UI_ORIGIN }),
  extractor: () => call("GET", "/api/production/extractor"),
  setExtractor: (config: ExtractorConfig) => call("PUT", "/api/production/extractor", { config, origin: UI_ORIGIN }),
  switchExtractor: (on: boolean) => call("POST", "/api/production/extractor/switch", { on, origin: UI_ORIGIN }),
  notifications: () => call("GET", "/api/notifications"),
  setNotifications: (targets: TargetDraft[], waitingDelayS: number) =>
    call("PUT", "/api/notifications", { targets, waiting_delay_s: waitingDelayS, origin: UI_ORIGIN }),
  testTarget: (id: string) => call<{ ok: boolean; result?: string }>("POST", "/api/notifications/test", { id, origin: UI_ORIGIN }),
};
