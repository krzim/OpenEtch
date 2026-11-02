// The preview's time slider (LightBurn's Preview window): a slider over the job's estimated time, from the engine's
// per-segment `t_s`, with play / pause at a speed relative to real time; scrubbing shows the toolpath burned up to
// that moment (the Viewport's `upTo`, drawn on the WebGL toolpath layer). "Start here" requests a run from the slider
// position (POST /api/runs `from_segment`; it still needs PIN approval), and "Save image" downloads the engine's
// rendered preview (GET /api/preview.png).

import { useEffect, useMemo, useRef, useState } from "react";
import { PLAYBACK_SPEEDS, clockText, segmentTimes, segmentsDoneAt, startHereSegment, timeOfSegments } from "../api/job.ts";
import { jobApi, saveBlob } from "../api/jobApi.ts";
import type { Preview } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { noPinMode } from "../api/runs.ts";
import { useLaserSource } from "../laser/laserContext.ts";
import { startCtxOf, startFlow, startState } from "../laser/startFlow.ts";
import { useLaserLive } from "../laser/useLaserLive.ts";
import { Icon } from "./ui.tsx";
import { HudItem } from "../hud/CanvasHud.tsx";
import type { PreviewOptions } from "./Viewport.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

export function PreviewTimeline(p: {
  preview: Preview;
  options: PreviewOptions;
  onOptions: (o: PreviewOptions) => void;
  /** Why runs cannot be requested (design-only device), or null. */
  designOnly: string | null;
  docName: string | null;
  notify: Notify;
  report: (e: unknown) => void;
  onRequested: () => void;
  /** No PIN on the engine: Start Here burns at once instead of requesting a run. */
  noPin?: boolean;
  /** The document revision this page shows (Start Here burns nothing when the engine's document is at another one). */
  docRevision?: number | null;
}) {
  const t = useT();
  const { preview, options, onOptions } = p;
  const times = useMemo(() => segmentTimes(preview), [preview]);
  const total = preview.segments?.length ?? 0;
  const duration = times.length ? times[times.length - 1] : 0;
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [busy, setBusy] = useState(false);
  const done = options.upTo === null ? total : Math.min(total, options.upTo);
  // The slider's own time while scrubbing or playing (between segment ends), else the end of the last shown segment.
  const [clock, setClock] = useState<number | null>(null);
  const at = clock ?? timeOfSegments(times, done);
  const optsRef = useRef(options);
  optsRef.current = options;
  const { facts } = useLaserLive();
  // The whole job's estimate for the Start label ("Start · all · ~2:14").
  const estimateS = typeof preview.stats?.estimated_time_s === "number" ? preview.stats.estimated_time_s : duration > 0 ? duration : null;
  useLaserSource("preview", { estimateS });

  // A new preview (an edit) resets the playhead's own clock.
  useEffect(() => setClock(null), [preview]);

  useEffect(() => {
    if (!playing || total === 0 || !(duration > 0)) return;
    let raf = 0;
    let last = performance.now();
    let pos = optsRef.current.upTo === null || optsRef.current.upTo >= total ? 0 : timeOfSegments(times, optsRef.current.upTo);
    const step = (now: number) => {
      pos += ((now - last) / 1000) * speed;
      last = now;
      if (pos >= duration) {
        setClock(null);
        onOptions({ ...optsRef.current, upTo: null });
        setPlaying(false);
        return;
      }
      setClock(pos);
      const n = segmentsDoneAt(times, pos);
      if (n !== optsRef.current.upTo) onOptions({ ...optsRef.current, upTo: n });
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [playing, total, duration, speed, times, onOptions]);

  if (total === 0) return null;
  const startAt = startHereSegment(preview, done);
  // Coarse enough for a long job, fine enough for a short one.
  const step = duration > 0 ? Math.max(0.001, duration / 2000) : 1;
  const scrub = (v: number) => {
    setPlaying(false);
    // The range snaps to whole steps, so its last stop can sit up to one step short of `duration`: that is the end.
    if (v + step > duration) {
      setClock(null);
      onOptions({ ...options, upTo: null });
      return;
    }
    setClock(v);
    onOptions({ ...options, upTo: segmentsDoneAt(times, v) });
  };
  // Start here goes through the one Start flow (laser/startFlow.ts): the same gates, refusals and checklist as Start.
  const startCtx = startCtxOf(facts, "startHere", {
    fromSegment: startAt,
    designOnly: p.designOnly,
    revision: p.docRevision ?? facts.revision,
    notify: (m) => p.notify(m),
    report: (e) => (e instanceof Error && !("status" in e) ? p.notify(e.message, "error") : p.report(e)),
    refreshMachine: p.onRequested,
  });
  const startSt = startState(startCtx);
  const startHere = () => {
    if (startAt === null) return;
    setBusy(true);
    void startFlow(startCtx).finally(() => setBusy(false));
  };
  const saveImage = () => {
    setBusy(true);
    jobApi
      .previewPng(3200, options.jumps)
      .then(({ blob, filename }) => {
        const base = p.docName ? p.docName.replace(/[\\/:*?"<>|]+/g, "_") : null;
        saveBlob(blob, base ? `${base} preview.png` : (filename ?? "preview.png"));
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  return (
    // The canvas HUD's bottom centre: it shrinks (and wraps to two rows) beside the corner controls, or stacks above them.
    // Optional: over a small phone's half-open Job sheet (which has its own Start) it gives way to the warnings and
    // toasts rather than have them cover it, and comes back when there is room.
    <HudItem id="pv-timeline" slot="bc" rank={20} minW={250} maxW={680} optional>
      <div className="pv-player pv-timeline" role="group" aria-label={t("pvt.label")}>
        <button
          className="icon-btn small"
          title={playing ? t("pvt.pause") : t("pvt.play")}
          aria-label={playing ? t("pvt.pause") : t("pvt.play")}
          onClick={() => {
            if (!playing && options.upTo === null) setClock(0);
            setPlaying((v) => !v);
          }}
        >
          <Icon name={playing ? "pause" : "play"} size={16} />
        </button>
        <input
          type="range"
          min={0}
          max={duration > 0 ? duration : total}
          step={step}
          value={duration > 0 ? Math.min(at, duration) : done}
          aria-label={t("pvt.time")}
          aria-valuetext={`${clockText(at)} / ${clockText(duration)}`}
          onChange={(e) => scrub(Number(e.target.value))}
        />
        <span className="pv-count" title={t("pvt.segments", { done, total })}>
          {clockText(Math.min(at, duration))} / {clockText(duration)}
        </span>
        <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))} aria-label={t("pvt.speed")} title={t("pvt.speedTitle")}>
          {PLAYBACK_SPEEDS.map((s) => (
            <option key={s} value={s}>
              {s}x
            </option>
          ))}
        </select>
        <button
          className="btn small btn-start btn-start-here"
          disabled={busy || startAt === null || !startSt.enabled}
          title={
            startSt.reason ??
            (startAt === null ? t("pvt.startHereEnd") : noPinMode(facts.machine) ? t("lz.startHereTitle", { segment: startAt }) : t("lz.requestHereTitle", { segment: startAt }))
          }
          onClick={startHere}
        >
          {t("pvt.startHere")}
        </button>
        <button className="icon-btn small" disabled={busy} title={t("pvt.saveImage")} aria-label={t("pvt.saveImage")} onClick={saveImage}>
          <Icon name="image" size={16} />
        </button>
      </div>
    </HudItem>
  );
}
