// The Laser window's job controls (LightBurn's Laser window and Live Framing job controls, docked Fusion-style):
// progress and time remaining (GET /api/job, refreshed by `job_progress` events every 500 ms while a job runs),
// Pause / Resume / End after this run, the run options for the next request (Repeat, Run continuously, Start Marking
// input with Require Full Pulse, Cut Selected Graphics) and the job counter (Count, Reset Count, Stop At).
//
// Nothing here fires the laser: "Request run" creates a pending run that still needs the operator PIN in the Runs card.
// Pause is never gated; Resume continues a run a person approved, so the engine only takes it from the UI.

import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import {
  IDLE_JOB,
  MAX_REPEAT,
  canPause,
  canResume,
  iterationText,
  jobStateKey,
  jobStatus,
  progressPct,
  repeating,
  runOptionsSummary,
  runRequestBody,
  stopAtReached,
  type JobStatus,
} from "../api/job.ts";
import { jobApi } from "../api/jobApi.ts";
import { formatDuration } from "../api/laser.ts";
import type { LinkState } from "../api/useEngine.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { tr } from "../workspace/tr.ts";
import { getRunOptions, setRunOptions, useRunOptions } from "./jobStore.ts";
import { Icon } from "./ui.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

export interface JobState {
  job: JobStatus;
  /** The engine has no job routes (older engine): the controls are shown disabled. */
  unsupported: boolean;
  refresh: () => void;
}

/** Machine events after which the job status is fetched again. */
const REFETCH_ON = new Set(["paused", "resumed", "job_count", "run_started", "run_finished", "run_approved", "awaiting_start_input", "stopped", "error"]);

/** GET /api/job, kept current by `job_progress` and the machine events; polled slowly while a job is active. */
export function useJobStatus(link: LinkState): JobState {
  const [job, setJob] = useState<JobStatus>(IDLE_JOB);
  const [unsupported, setUnsupported] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const refresh = useCallback(() => {
    jobApi
      .job()
      .then((r) => {
        if (!alive.current) return;
        setJob(jobStatus(r));
        setUnsupported(false);
      })
      .catch((e: unknown) => {
        if (alive.current && e instanceof ApiError && (e.status === 404 || e.status === 405)) setUnsupported(true);
      });
  }, []);
  useEffect(() => {
    if (link !== "closed") refresh();
  }, [link, refresh]);
  useEffect(
    () =>
      onEngineEvent((ev) => {
        if (ev.type === "job_progress") setJob(jobStatus(ev.status));
        else if (ev.type === "run_finished" || (ev.type === "machine" && REFETCH_ON.has(String(ev.event)))) refresh();
      }),
    [refresh],
  );
  // The socket can drop events; while a job is active, refetch every few seconds.
  useEffect(() => {
    if (!job.active) return;
    const id = window.setInterval(refresh, 4000);
    return () => window.clearInterval(id);
  }, [job.active, refresh]);
  return { job, unsupported, refresh };
}

/** Request a run with the Laser window's options (and the selection for Cut Selected Graphics). */
export async function requestRunWithOptions(selection: readonly string[], fromSegment: number | null = null): Promise<string> {
  // Beginner Mode hides Cut Selected Graphics and ignores it (LightBurn ignores hidden optional settings).
  const o = getRunOptions();
  const body = runRequestBody(fromSegment === null && !getPrefs().beginnerMode ? o : { ...o, cutSelected: false }, { selection, fromSegment });
  await jobApi.requestRun(body);
  return fromSegment === null ? tr("rest.runRequested") : tr("rest.runFromRequested", { n: fromSegment });
}

/** Pause a running job, or resume a paused one (the Pause / resume hotkey). Resolves to the toast text. */
export async function togglePause(): Promise<string> {
  const j = jobStatus(await jobApi.job());
  if (canResume(j)) {
    await jobApi.resume();
    return "Job resumed.";
  }
  if (canPause(j)) {
    await jobApi.pause();
    return "Pausing the job.";
  }
  throw new Error("No job is running.");
}

/** Progress bar, state, iteration, times and the Pause / Resume / End after this run buttons. */
export function JobProgress(p: { state: JobState; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const { job, unsupported } = p.state;
  const [busy, setBusy] = useState(false);
  const act = (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true);
    fn()
      .then(() => p.notify(ok))
      .catch(p.report)
      .finally(() => {
        setBusy(false);
        p.state.refresh();
      });
  };
  const st = jobStateKey(job);
  const it = iterationText(job);
  const pct = progressPct(job);
  const notYet = unsupported ? t("laser.notYet") : undefined;
  return (
    <div className="job-progress" aria-label={t("job.progress")}>
      <div className="job-progress-head">
        <span className={`job-state job-state-${job.paused ? "paused" : job.active ? "active" : "idle"}`} role="status">
          {t(st.key)}
        </span>
        {it && <span className="job-iter">{it}</span>}
        {job.active && job.parts > 1 && <span className="job-iter">{t("job.part", { part: Math.max(1, job.part), parts: job.parts })}</span>}
      </div>
      <div className="job-bar" title={notYet}>
        <progress max={100} value={job.active ? pct : 0} aria-label={t("job.progress")} />
        <span className="job-pct">{job.active ? `${pct.toFixed(0)}%` : "—"}</span>
      </div>
      {job.active && (
        <div className="job-times">
          <span>{t("job.elapsed", { time: formatDuration(job.elapsed_s) })}</span>
          <span>{t("job.remaining", { time: formatDuration(job.remaining_s) })}</span>
          {job.packets_total > 0 && <span className="muted">{t("job.packets", { sent: job.packets_sent, total: job.packets_total })}</span>}
        </div>
      )}
      <div className="laser-buttons">
        <span title={notYet}>
          <button
            className="btn"
            disabled={unsupported || busy || !canPause(job)}
            aria-label={t("laser.pause")}
            title={t("job.pauseTitle")}
            onClick={() => act(jobApi.pause, t("job.pausedToast"))}
          >
            <Icon name="pause" size={18} /> {t("laser.pause")}
          </button>
        </span>
        <span title={notYet}>
          <button
            className="btn"
            disabled={unsupported || busy || !canResume(job)}
            aria-label={t("laser.resume")}
            title={t("job.resumeTitle")}
            onClick={() => act(jobApi.resume, t("job.resumedToast"))}
          >
            <Icon name="play" size={18} /> {t("laser.resume")}
          </button>
        </span>
        {repeating(job) && (
          <button
            className="btn"
            disabled={busy || job.end_after_current}
            title={t("job.endAfterTitle")}
            onClick={() => act(jobApi.endAfterCurrent, t("job.endAfterToast"))}
          >
            {job.end_after_current ? t("job.endingAfter") : t("job.endAfter")}
          </button>
        )}
      </div>
    </div>
  );
}

/** Repeat, Run continuously, Start Marking input (+ Require Full Pulse) and Cut Selected Graphics. */
export function RunOptionsPanel(p: { selected: number; hasStartInput: boolean | null }) {
  const t = useT();
  const beginnerMode = usePrefs().beginnerMode;
  const o = useRunOptions();
  const [repeatText, setRepeatText] = useState(String(o.repeat));
  useEffect(() => setRepeatText(String(o.repeat)), [o.repeat]);
  const commitRepeat = () => {
    const v = Math.round(Number(repeatText));
    if (Number.isFinite(v) && v >= 1) setRunOptions({ repeat: Math.min(MAX_REPEAT, v) });
    else setRepeatText(String(o.repeat));
  };
  return (
    <div className="job-options" role="group" aria-label={t("job.options")}>
      <label className="job-row" title={t("job.repeatTitle")}>
        <span>{t("job.repeat")}</span>
        <input
          type="text"
          inputMode="numeric"
          className="job-num"
          value={repeatText}
          disabled={o.continuous}
          aria-label={t("job.repeat")}
          onChange={(e) => setRepeatText(e.target.value.replace(/[^\d]/g, ""))}
          onBlur={commitRepeat}
          onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
        />
        <span className="muted">{t("job.times")}</span>
      </label>
      <label className="laser-check" title={t("job.continuousTitle")}>
        <input type="checkbox" checked={o.continuous} onChange={(e) => setRunOptions({ continuous: e.target.checked })} />
        {t("job.continuous")}
      </label>
      <label className="laser-check" title={p.hasStartInput === false ? t("job.noStartInput") : t("job.startInputTitle")}>
        <input type="checkbox" checked={o.startInput} disabled={p.hasStartInput === false && !o.startInput} onChange={(e) => setRunOptions({ startInput: e.target.checked })} />
        {t("job.startInput")}
      </label>
      {o.startInput && (
        <label className="laser-check job-indent" title={t("job.fullPulseTitle")}>
          <input type="checkbox" checked={o.fullPulse} onChange={(e) => setRunOptions({ fullPulse: e.target.checked })} />
          {t("job.fullPulse")}
        </label>
      )}
      {!beginnerMode && (
        <label className="laser-check" title={t("job.cutSelectedTitle")}>
          <input type="checkbox" checked={o.cutSelected} onChange={(e) => setRunOptions({ cutSelected: e.target.checked })} />
          {t("job.cutSelected")}
          {o.cutSelected && <span className="pill">{p.selected}</span>}
        </label>
      )}
      <p className="hint job-summary">{t("job.nextRun", { summary: runOptionsSummary(beginnerMode ? { ...o, cutSelected: false } : o, p.selected) })}</p>
    </div>
  );
}

/** LightBurn's Count / Reset Count / Stop At. */
export function JobCounter(p: { state: JobState; report: (e: unknown) => void }) {
  const t = useT();
  const { job, unsupported } = p.state;
  const [text, setText] = useState(job.stop_at === null ? "" : String(job.stop_at));
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(job.stop_at === null ? "" : String(job.stop_at));
  }, [job.stop_at]);
  const commit = () => {
    editing.current = false;
    const tt = text.trim();
    const v = tt === "" ? null : Math.round(Number(tt));
    if (v !== null && !(Number.isFinite(v) && v >= 0)) {
      setText(job.stop_at === null ? "" : String(job.stop_at));
      return;
    }
    if (v === job.stop_at) return;
    jobApi
      .setStopAt(v)
      .catch(p.report)
      .finally(() => p.state.refresh());
  };
  const reached = stopAtReached(job);
  return (
    <div className="job-counter" title={unsupported ? t("laser.notYet") : undefined}>
      <span className="job-count-label">{t("job.count")}</span>
      <span className="job-count" aria-live="polite">
        {job.count}
      </span>
      <button className="btn small" disabled={unsupported || job.active} title={t("job.resetCountTitle")} onClick={() => void jobApi.resetCount().catch(p.report).finally(p.state.refresh)}>
        {t("job.resetCount")}
      </button>
      <label className="job-row" title={t("job.stopAtTitle")}>
        <span>{t("job.stopAt")}</span>
        <input
          type="text"
          inputMode="numeric"
          className="job-num"
          placeholder={t("job.off")}
          value={text}
          disabled={unsupported}
          onFocus={() => (editing.current = true)}
          onChange={(e) => setText(e.target.value.replace(/[^\d]/g, ""))}
          onBlur={commit}
          onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
        />
      </label>
      {reached && (
        <p className="run-hint job-wide" role="status">
          {t("job.stopAtReached", { count: job.count })}
        </p>
      )}
    </div>
  );
}
