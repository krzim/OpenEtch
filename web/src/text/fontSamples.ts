// Live font samples for the font picker: each row asks for its sample when it scrolls into view, the requests are
// gathered for a moment and sent as `font.preview` calls of up to 32 fonts (crates/commands/src/builtin/
// font_preview.rs), and the answers are kept for the page. The engine draws the samples because the browser cannot:
// Hershey, SHX and JSF fonts are not web fonts, and the iPad does not have the Pi's installed fonts. Pure parts are
// tested in fontSamples.test.ts.

import { execute } from "../api/client.ts";
import type { FontKind } from "../api/types.ts";
import { TXT } from "./textModel.ts";

export type Sample = { d: string; viewBox: [number, number, number, number] } | { error: string };

/** Fonts per `font.preview` call (the engine takes 64). */
export const BATCH = 32;
/** How long requests are gathered before they go out, ms. */
export const GATHER_MS = 60;
/** The sample when the picker has none of its own. */
export const DEFAULT_SAMPLE = "AaBbCc 123";
/** Longest sample the engine draws. */
export const MAX_SAMPLE = 40;

export function sampleKey(f: { family: string; kind: FontKind }, text: string): string {
  return `${f.kind}\u0000${f.family.toLowerCase()}\u0000${text}`;
}

/** The sample text for `text` (the picker's own or the selected text's first line): one line, at most 40 chars. */
export function sampleText(text: string | null | undefined): string {
  const line = (text ?? "").split("\n").find((l) => l.trim()) ?? "";
  const one = line.replace(/\s+/g, " ").trim();
  return one ? [...one].slice(0, MAX_SAMPLE).join("") : DEFAULT_SAMPLE;
}

/** `font.preview`'s answer as samples by key (entries that are not usable are left out). */
export function parsePreview(r: unknown, text: string): Map<string, Sample> {
  const out = new Map<string, Sample>();
  if (!Array.isArray(r)) return out;
  for (const e of r) {
    if (!e || typeof e !== "object") continue;
    const x = e as Record<string, unknown>;
    if (typeof x.family !== "string" || typeof x.kind !== "string") continue;
    const key = sampleKey({ family: x.family, kind: x.kind as FontKind }, text);
    const vb = Array.isArray(x.view_box) && x.view_box.length === 4 && x.view_box.every((n) => typeof n === "number" && Number.isFinite(n)) ? (x.view_box as [number, number, number, number]) : null;
    if (typeof x.d === "string" && vb && vb[2] > 0 && vb[3] > 0) out.set(key, { d: x.d, viewBox: vb });
    else if (typeof x.error === "string") out.set(key, { error: x.error });
  }
  return out;
}

/** Split queued requests into calls: by sample text, at most BATCH fonts each. */
export function batches(queue: readonly { font: { family: string; kind: FontKind }; text: string }[]): { text: string; fonts: { family: string; kind: FontKind }[] }[] {
  const by = new Map<string, { family: string; kind: FontKind }[]>();
  for (const q of queue) {
    const list = by.get(q.text) ?? [];
    if (!list.some((f) => f.kind === q.font.kind && f.family.toLowerCase() === q.font.family.toLowerCase())) list.push(q.font);
    by.set(q.text, list);
  }
  const out: { text: string; fonts: { family: string; kind: FontKind }[] }[] = [];
  for (const [text, fonts] of by) for (let i = 0; i < fonts.length; i += BATCH) out.push({ text, fonts: fonts.slice(i, i + BATCH) });
  return out;
}

// ---- the page's cache and request queue -----------------------------------------------------------------------

const cache = new Map<string, Sample>();
const inFlight = new Set<string>();
let queue: { font: { family: string; kind: FontKind }; text: string }[] = [];
let timer: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<() => void>();
let enabled = true;

export function cachedSample(f: { family: string; kind: FontKind }, text: string): Sample | undefined {
  return cache.get(sampleKey(f, text));
}

export function subscribeSamples(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function flush(): void {
  timer = null;
  const work = batches(queue);
  queue = [];
  for (const b of work) {
    void execute({ id: TXT.fontPreview, params: { fonts: b.fonts, text: b.text } })
      .then((r) => {
        for (const [k, v] of parsePreview(r.result, b.text)) cache.set(k, v);
      })
      .catch(() => {
        // An engine without font.preview: stop asking; the rows keep their names in the browser's font.
        enabled = false;
      })
      .finally(() => {
        for (const f of b.fonts) inFlight.delete(sampleKey(f, b.text));
        listeners.forEach((fn) => fn());
      });
  }
}

/** Ask for the sample of `f` showing `text` (no-op when cached or on its way). */
export function requestSample(f: { family: string; kind: FontKind }, text: string): void {
  const key = sampleKey(f, text);
  if (!enabled || cache.has(key) || inFlight.has(key)) return;
  inFlight.add(key);
  queue.push({ font: { family: f.family, kind: f.kind }, text });
  timer ??= setTimeout(flush, GATHER_MS);
}

/** Forget every sample (after Reload: new fonts may draw differently). */
export function clearSamples(): void {
  cache.clear();
  enabled = true;
  listeners.forEach((fn) => fn());
}
