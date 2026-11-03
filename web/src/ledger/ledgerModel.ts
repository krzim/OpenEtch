// The job ledger's model (short list #5; engine: crates/engine/src/ledger.rs, routes in api_ledger.rs): every run
// that ended with the settings each layer burned with, its share of the time, the serial number / CSV values, the
// verdict (OK / scrap) and note, and whether a thumbnail and the burned document are kept (Re-run). Pure: parsing,
// the search query string, short texts and totals, so `npm test` covers what the Job log window shows.

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
const optStr = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

export type Verdict = "ok" | "scrap";

/** One layer as it burned. `settings` and `subLayers` are the engine's CutSettings JSON. */
export interface LedgerLayer {
  id: string;
  name: string;
  color: string;
  output: boolean;
  objects: number;
  /** Of those, images. */
  images: number;
  settings: Record<string, unknown>;
  subLayers: Record<string, unknown>[];
  preset: string | null;
  passes: number;
  estimatedS: number;
  markMm: number;
}

export interface LedgerVariables {
  current: number;
  csvFile: string | null;
  texts: { object: string; name: string; mode: string; value: string }[];
}

export interface LedgerEntry {
  id: string;
  timeUnixS: number;
  runId: string;
  /** finished, failed, aborted or cancelled. */
  state: string;
  origin: string;
  device: string;
  project: string | null;
  document: string | null;
  summary: string;
  durationS: number;
  completed: number;
  error: string | null;
  layers: LedgerLayer[];
  variables: LedgerVariables | null;
  estimatedS: number | null;
  markTimeS: number | null;
  options: Record<string, unknown> | null;
  verdict: Verdict | null;
  note: string | null;
  notedBy: string | null;
  notedUnixS: number | null;
  thumbnail: boolean;
  rerun: boolean;
  rerunReason: string | null;
}

function layerOf(v: unknown): LedgerLayer | null {
  const o = obj(v);
  if (!o) return null;
  return {
    id: str(o.id),
    name: str(o.name),
    color: str(o.color) || "#888888",
    output: o.output !== false,
    objects: isNum(o.objects) ? o.objects : 0,
    images: isNum(o.images) ? o.images : 0,
    settings: obj(o.settings) ?? {},
    subLayers: Array.isArray(o.sub_layers) ? o.sub_layers.flatMap((s) => (obj(s) ? [obj(s) as Record<string, unknown>] : [])) : [],
    preset: optStr(o.preset),
    passes: isNum(o.passes) ? o.passes : 0,
    estimatedS: isNum(o.estimated_time_s) ? o.estimated_time_s : 0,
    markMm: isNum(o.mark_length_mm) ? o.mark_length_mm : 0,
  };
}

function variablesOf(v: unknown): LedgerVariables | null {
  const o = obj(v);
  if (!o) return null;
  const texts = Array.isArray(o.texts)
    ? o.texts.flatMap((t) => {
        const x = obj(t);
        return x ? [{ object: str(x.object), name: str(x.name), mode: str(x.mode), value: str(x.value) }] : [];
      })
    : [];
  return { current: isNum(o.current) ? o.current : 0, csvFile: optStr(o.csv_file), texts };
}

/** One entry of `GET /api/job-log` (also an older engine's plain job log line). */
export function parseEntry(v: unknown): LedgerEntry | null {
  const o = obj(v);
  if (!o) return null;
  const time = isNum(o.time_unix_s) ? o.time_unix_s : 0;
  const runId = str(o.run_id);
  const verdict = o.verdict === "ok" || o.verdict === "scrap" ? o.verdict : null;
  return {
    id: str(o.id) || `${time}-${runId}`,
    timeUnixS: time,
    runId,
    state: str(o.state),
    origin: str(o.origin),
    device: str(o.device),
    project: optStr(o.project),
    document: optStr(o.document),
    summary: str(o.summary),
    durationS: isNum(o.duration_s) ? o.duration_s : 0,
    completed: isNum(o.completed) ? o.completed : 0,
    error: optStr(o.error),
    layers: Array.isArray(o.layers) ? o.layers.flatMap((l) => layerOf(l) ?? []) : [],
    variables: variablesOf(o.variables),
    estimatedS: isNum(o.estimated_time_s) ? o.estimated_time_s : null,
    markTimeS: isNum(o.mark_time_s) ? o.mark_time_s : null,
    options: obj(o.options),
    verdict,
    note: optStr(o.note),
    notedBy: optStr(o.noted_by),
    notedUnixS: isNum(o.noted_unix_s) ? o.noted_unix_s : null,
    thumbnail: o.thumbnail === true,
    rerun: o.rerun === true,
    rerunReason: optStr(o.rerun_reason),
  };
}

/** `GET /api/job-log` `entries`, newest first. */
export function parseLedger(raw: unknown): LedgerEntry[] {
  const v = obj(raw);
  const list = Array.isArray(v?.entries) ? v.entries : Array.isArray(raw) ? raw : [];
  return list.flatMap((e) => parseEntry(e) ?? []).sort((a, b) => b.timeUnixS - a.timeUnixS || b.id.localeCompare(a.id));
}

export interface LedgerFilter {
  q: string;
  verdict: "any" | Verdict | "unmarked";
  state: "any" | "finished" | "failed" | "aborted" | "cancelled";
  limit: number;
}

export const DEFAULT_FILTER: LedgerFilter = { q: "", verdict: "any", state: "any", limit: 200 };

/** The query string of `GET /api/job-log` for a filter (`format=csv` for the CSV). */
export function ledgerQuery(f: LedgerFilter, format?: "csv"): string {
  const p = new URLSearchParams();
  p.set("limit", String(Math.max(1, Math.min(2000, Math.round(f.limit)))));
  const q = f.q.trim();
  if (q) p.set("q", q);
  if (f.verdict !== "any") p.set("verdict", f.verdict);
  if (f.state !== "any") p.set("state", f.state);
  if (format) p.set("format", format);
  return p.toString();
}

/** A number without trailing zeros: 40, 0.5, 1000. */
export function num(v: number, digits = 3): string {
  return String(Number(v.toFixed(digits)));
}

const n = (s: Record<string, unknown>, k: string): number | null => (isNum(s[k]) ? (s[k] as number) : null);

/** The settings that matter for a MOPA mark, as short parts: ["40 %", "1000 mm/s", "30 kHz", "200 ns", "×2"]. */
export function settingParts(s: Record<string, unknown>): string[] {
  const out: string[] = [];
  const power = n(s, "power_pct");
  const speed = n(s, "speed_mm_s");
  const freq = n(s, "frequency_khz");
  const pulse = n(s, "pulse_width_ns");
  const passes = n(s, "passes");
  if (power !== null) out.push(`${num(power, 2)} %`);
  if (speed !== null) out.push(`${num(speed, 1)} mm/s`);
  if (freq !== null) out.push(`${num(freq, 2)} kHz`);
  if (pulse !== null) out.push(`${num(pulse, 1)} ns`);
  if (passes !== null && passes > 1) out.push(`×${passes}`);
  return out;
}

/** The layer's mode as the engine names it (line, fill, fill_then_line, offset_fill), plus the image dither for images. */
export function layerMode(s: Record<string, unknown>): string {
  return str(s.mode) || "line";
}

/** The image dither of a layer's settings (`jarvis`, `grayscale`, `slice3d`, ...), if set. */
export function imageDither(s: Record<string, unknown>): string | null {
  return optStr(obj(s.image)?.dither);
}

/** The hatch line interval (fills), mm. */
export function intervalOf(s: Record<string, unknown>): number | null {
  const h = obj(s.hatch);
  return h && isNum(h.interval_mm) ? h.interval_mm : null;
}

/** The image line interval, mm. */
export function imageIntervalOf(s: Record<string, unknown>): number | null {
  const h = obj(s.image);
  return h && isNum(h.interval_mm) ? h.interval_mm : null;
}

/** Whether a layer's settings hatch (fill, fill then line, offset fill) rather than only trace outlines. */
export function hatches(s: Record<string, unknown>): boolean {
  return ["fill", "fill_then_line", "offset_fill"].includes(str(s.mode));
}

/** Totals over the rows shown. */
export function ledgerTotals(rows: readonly LedgerEntry[]): { runs: number; finished: number; failed: number; ok: number; scrap: number; seconds: number } {
  return {
    runs: rows.length,
    finished: rows.filter((r) => r.state === "finished").length,
    failed: rows.filter((r) => r.state === "failed" || r.state === "aborted").length,
    ok: rows.filter((r) => r.verdict === "ok").length,
    scrap: rows.filter((r) => r.verdict === "scrap").length,
    seconds: rows.reduce((s, r) => s + r.durationS, 0),
  };
}

/** The entry's title: its project, else its document's name, else the run id. */
export function entryTitle(e: LedgerEntry): string {
  return e.project ?? e.document ?? e.runId;
}

/** The run options worth showing ("Repeat 3", "Continuous", "Selected graphics", "From segment 120", "Batch of 4"). */
export function optionParts(o: Record<string, unknown> | null): string[] {
  if (!o) return [];
  const out: string[] = [];
  if (isNum(o.repeat) && o.repeat > 1) out.push(`repeat:${o.repeat}`);
  if (o.continuous === true) out.push("continuous");
  if (o.start_input === true) out.push("start_input");
  if (Array.isArray(o.selection)) out.push(`selection:${o.selection.length}`);
  if (isNum(o.from_segment)) out.push(`from_segment:${o.from_segment}`);
  if (Array.isArray(o.queue)) out.push(`queue:${o.queue.length}`);
  if (obj(o.table)) out.push("table");
  return out;
}

/** How far the board's own time is from the estimate, in % (null when it did not report one). */
export function estimateError(e: LedgerEntry): number | null {
  if (e.markTimeS === null || e.estimatedS === null || !(e.estimatedS > 0)) return null;
  return Math.round(((e.markTimeS - e.estimatedS) / e.estimatedS) * 100);
}

/** Each layer's share of the job's estimate, 0..1 (the layers' own sum is the whole). */
export function timeShares(layers: readonly LedgerLayer[]): number[] {
  const total = layers.reduce((s, l) => s + l.estimatedS, 0);
  return layers.map((l) => (total > 0 ? l.estimatedS / total : 0));
}

export interface LedgerSettings {
  keepDocuments: number;
  maxDocumentMb: number;
  keepThumbnails: number;
  thumbnailPx: number;
}

export interface LedgerUsage {
  thumbnails: number;
  thumbnailBytes: number;
  documents: number;
  documentBytes: number;
}

/** `GET /api/job-ledger/settings`. */
export function parseLedgerSettings(raw: unknown): { settings: LedgerSettings; usage: LedgerUsage } {
  const v = obj(raw);
  const s = obj(v?.settings) ?? {};
  const u = obj(v?.usage) ?? {};
  const k = (o: Record<string, unknown>, key: string, d: number) => (isNum(o[key]) ? (o[key] as number) : d);
  return {
    settings: {
      keepDocuments: k(s, "keep_documents", 100),
      maxDocumentMb: k(s, "max_document_mb", 25),
      keepThumbnails: k(s, "keep_thumbnails", 2000),
      thumbnailPx: k(s, "thumbnail_px", 160),
    },
    usage: { thumbnails: k(u, "thumbnails", 0), thumbnailBytes: k(u, "thumbnail_bytes", 0), documents: k(u, "documents", 0), documentBytes: k(u, "document_bytes", 0) },
  };
}

/** The PUT body of `/api/job-ledger/settings`. */
export function settingsBody(s: LedgerSettings): Record<string, unknown> {
  return { keep_documents: s.keepDocuments, max_document_mb: s.maxDocumentMb, keep_thumbnails: s.keepThumbnails, thumbnail_px: s.thumbnailPx, origin: "ui" };
}

/** "1.2 MB", "340 kB", "12 B". */
export function bytesText(b: number): string {
  if (!(b > 0)) return "0 B";
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${num(b / 1024, 0)} kB`;
  if (b < 1024 * 1024 * 1024) return `${num(b / (1024 * 1024), 1)} MB`;
  return `${num(b / (1024 * 1024 * 1024), 2)} GB`;
}

/** What `POST /api/job-log/{id}/rerun` answered, for the toast: the pending run, or why no run was requested. */
export function rerunOutcome(raw: unknown): { documentId: string | null; runId: string | null; problem: string | null } {
  const v = obj(raw);
  const run = obj(v?.run);
  return {
    documentId: optStr(v?.document_id),
    runId: optStr(run?.id),
    problem: optStr(v?.run_error) ?? optStr(v?.device_mismatch),
  };
}
