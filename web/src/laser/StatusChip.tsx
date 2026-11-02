// The machine status chip left of STOP in every layout (owner: P5, docs/ux/ui-plan-2026-11.md P5): Idle, Framing (pulse
// plus an inline Stop frame), Running 42% · 3:12 (plus Pause), Paused (plus Resume), Offline. A tap opens the Laser
// panel or the Job sheet. `compact` (phone): just the status dot, still a button. While framing, Esc stops the frame.

import { useCallback } from "react";
import { jobApi } from "../api/jobApi.ts";
import { useJobStatus } from "../components/JobControls.tsx";
import { Icon } from "../components/ui.tsx";
import { useSharedApprovalGates } from "../libs/JobSafety.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { useEscStopsFraming } from "./escFraming.ts";
import { chipState } from "./statusChip.ts";
import { useLaserLive } from "./useLaserLive.ts";
import "./laser.css";

export interface StatusChipProps {
  /** Phone: just the status dot. */
  compact?: boolean;
}

export function StatusChip({ compact = false }: StatusChipProps) {
  const t = useT();
  const { facts } = useLaserLive();
  const job = useJobStatus(facts.link);
  // Keeps the Job Checklist gates known to the Start flow (the hotkey decides from them) wherever the chip is mounted.
  useSharedApprovalGates();
  const st = chipState(facts.machine, facts.link, job.unsupported ? null : job.job, facts.httpError);
  const refresh = useCallback(() => {
    facts.refreshMachine();
    job.refresh();
  }, [facts, job]);
  const afterFrameStop = useCallback(
    (err: unknown | null) => {
      if (err) facts.report(err);
      else facts.notify(t("lz.frameStopped"));
      refresh();
    },
    [facts, refresh, t],
  );
  useEscStopsFraming(st.kind === "framing", afterFrameStop);

  const act = () => {
    const call = st.action === "stopFrame" ? jobApi.frameStop() : st.action === "pause" ? jobApi.pause() : st.action === "resume" ? jobApi.resume() : null;
    if (!call) return;
    call
      .then(() => facts.notify(st.action === "stopFrame" ? t("lz.frameStopped") : st.action === "pause" ? t("lz.paused") : t("lz.resumed")))
      .catch(facts.report)
      .finally(refresh);
  };
  const full = st.pending > 0 ? `${st.text} · ${t("lz.chipPending", { n: st.pending })}` : st.text;
  const label = t("lz.chipTitle", { state: full });
  const cls = `status-chip tone-${st.tone} kind-${st.kind}${st.pulse ? " pulse" : ""}${compact ? " compact" : ""}`;
  return (
    <div className={cls} data-kind={st.kind}>
      <button className="status-chip-main" onClick={() => facts.openLaser()} title={label} aria-label={label}>
        <span className="status-chip-dot" aria-hidden="true" />
        {!compact && <span className="status-chip-text">{st.text}</span>}
        {!compact && st.pending > 0 && <span className="status-chip-count">{st.pending}</span>}
        {!compact && st.pct !== null && (
          <span className="status-chip-bar" aria-hidden="true">
            <span style={{ width: `${st.pct}%` }} />
          </span>
        )}
      </button>
      {!compact && st.action && (
        <button
          className="status-chip-act"
          onClick={act}
          title={st.action === "stopFrame" ? t("lz.stopFrame") : st.action === "pause" ? t("lz.pause") : t("lz.resume")}
          aria-label={st.action === "stopFrame" ? t("lz.stopFrame") : st.action === "pause" ? t("lz.pause") : t("lz.resume")}
        >
          <Icon name={st.action === "stopFrame" ? "frameStop" : st.action === "pause" ? "pause" : "play"} size={16} />
          <span className="status-chip-act-text">{st.action === "stopFrame" ? t("lz.stopFrame") : st.action === "pause" ? t("lz.pause") : t("lz.resume")}</span>
        </button>
      )}
    </div>
  );
}
