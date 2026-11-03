// The preview and its stats, planned in the browser (previewClient.ts, crates/webcore in a Web Worker) from the
// document the app already has and the engine's small planning context (GET /api/preview/context), instead of
// downloading GET /api/preview (tens of MB for an engraved image) after every edit.
//
// - Laser workspace (`wantPreview`): the packed preview for the toolpath layer, timeline and stats card.
// - Design workspace (`wantStats`): the summary only (time estimate), a moment after the last edit.
// - `contextSeq` changes when the context does (device_changed, run_finished: the job counter and the learned
//   estimate correction).
//
// `mode` is "fallback" when this browser cannot run the worker or the engine has no context route: the caller then
// uses GET /api/preview as before. Display only: runs are planned by the engine from its own document.

import { useEffect, useRef, useState } from "react";
import { api, ApiError } from "../api/client.ts";
import { previewFromWorker, type PreviewSummary } from "../api/packedPreview.ts";
import type { GDocument, Preview } from "../api/types.ts";
import { Stale, fillCore, previewCore, workerSupported } from "./previewClient.ts";

export type BrowserPreviewMode = "starting" | "worker" | "fallback";

export interface BrowserPreview {
  mode: BrowserPreviewMode;
  preview: Preview | null;
  /** Why the document cannot be planned (the engine's 422 text), or null. */
  previewError: string | null;
  /** The latest summary (from a stats job, or the full preview's). */
  summary: PreviewSummary | null;
  /** The revision `summary` is of. */
  summaryRev: number | null;
}

/** Wait this long after the last edit before planning stats in the Design workspace. */
const STATS_DELAY_MS = 350;

export function useBrowserPreview(o: { doc: GDocument | null; wantPreview: boolean; wantStats: boolean; contextSeq: number }): BrowserPreview {
  const [mode, setMode] = useState<BrowserPreviewMode>(workerSupported() ? "starting" : "fallback");
  const [ctxVersion, setCtxVersion] = useState(0);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [summary, setSummary] = useState<{ rev: number; s: PreviewSummary | null } | null>(null);
  const modeRef = useRef(mode);
  modeRef.current = mode;
  // What the shown preview / summary was planned from: [revision, context version].
  const fullKey = useRef("");
  const statsKey = useRef("");

  // The planning context: at start, and whenever the engine says it changed.
  useEffect(() => {
    if (!workerSupported()) {
      setMode("fallback");
      return;
    }
    let live = true;
    void (async () => {
      let ctx: unknown;
      try {
        ctx = await api.previewContext();
      } catch (e) {
        // An engine without the route: the engine's preview, for good. Anything else keeps the old context.
        if (live && (modeRef.current === "starting" || (e instanceof ApiError && e.status === 404))) setMode("fallback");
        return;
      }
      const json = JSON.stringify(ctx);
      previewCore.setContext(json);
      fillCore.setContext(json);
      const ok = await previewCore.ready();
      if (!live) return;
      if (!ok) {
        setMode("fallback");
        return;
      }
      setMode("worker");
      setCtxVersion((v) => v + 1);
    })();
    return () => {
      live = false;
    };
  }, [o.contextSeq]);

  const doc = o.doc;
  const rev = doc?.revision ?? null;
  useEffect(() => {
    if (mode !== "worker" || !doc || rev === null) return;
    previewCore.setDocument(rev, doc);
    fillCore.setDocument(rev, doc);
    const key = `${rev}:${ctxVersion}`;
    let live = true;
    if (o.wantPreview) {
      if (fullKey.current === key) return;
      previewCore
        .run("full")
        .then((r) => {
          if (!live) return;
          const w = previewFromWorker(r.json, r.arrays);
          fullKey.current = key;
          statsKey.current = key;
          setPreview(w.preview);
          setPreviewError(w.error);
          setSummary({ rev: r.rev, s: w.summary });
        })
        .catch((e: unknown) => {
          if (!live || e instanceof Stale) return;
          if (!previewCore.ok()) setMode("fallback");
        });
      return () => {
        live = false;
      };
    }
    if (o.wantStats && statsKey.current !== key) {
      const timer = window.setTimeout(() => {
        previewCore
          .run("stats")
          .then((r) => {
            if (!live) return;
            const w = previewFromWorker(r.json);
            statsKey.current = key;
            setSummary({ rev: r.rev, s: w.error ? null : w.summary });
          })
          .catch((e: unknown) => {
            if (!live || e instanceof Stale) return;
            if (!previewCore.ok()) setMode("fallback");
          });
      }, STATS_DELAY_MS);
      return () => {
        live = false;
        window.clearTimeout(timer);
      };
    }
    return undefined;
    // `doc` is read through its revision: revisions are unique across the engine's open documents.
  }, [mode, rev, ctxVersion, o.wantPreview, o.wantStats]);

  return { mode, preview, previewError, summary: summary?.s ?? null, summaryRev: summary?.rev ?? null };
}

/**
 * Fill Region hover in the worker: the `path.fill_region_preview` result for `params` on document revision `rev`, or
 * null when the worker cannot answer (not running, another revision, options only the engine takes), and the caller
 * asks the engine.
 */
export async function workerFillPreview(rev: number, params: Record<string, unknown>): Promise<unknown | null> {
  if (!fillCore.ok() || !(await fillCore.ready())) return null;
  try {
    const r = await fillCore.run("fill", params);
    if (r.rev !== rev) return null;
    const v = JSON.parse(r.json) as { error?: unknown };
    if (v && typeof v === "object" && typeof v.error === "string") return null;
    return v;
  } catch {
    return null;
  }
}
