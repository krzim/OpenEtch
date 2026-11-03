// The Start button (owner: P5, docs/ux/ui-plan-2026-11.md P5): amber, smaller than STOP, its disabled reason as visible
// text, the scope and time in its label; it calls laser/startFlow.ts. P2 places the toolbar variant in the Laser
// ribbon, the Laser dock uses "dock" and the phone's Job sheet "sheet" (which also shows the Job Checklist inline).
// A refused Start stays above the button as an alert until it is dismissed or Start is pressed again.

import { noPinMode } from "../api/runs.ts";
import { Icon } from "../components/ui.tsx";
import { ApprovalChecks, useSharedApprovalGates } from "../libs/JobSafety.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { setLaserFlow } from "./laserContext.ts";
import { startCtxOf, startFlow, startState } from "./startFlow.ts";
import { useLaserLive } from "./useLaserLive.ts";
import { PreflightNotes } from "../jigs/Preflight.tsx";
import { useRemarkRegion } from "../jigs/regionStore.ts";
import "./laser.css";

export type StartButtonVariant = "toolbar" | "dock" | "sheet";

export interface StartButtonProps {
  variant: StartButtonVariant;
}

/** The refusal of the last Start, until dismissed or the next Start. */
export function StartRefusal({ text }: { text: string | null }) {
  const t = useT();
  if (!text) return null;
  return (
    <div className="start-refusal" role="alert">
      <span className="start-refusal-text">
        <strong>{t("lz.refused")}:</strong> {text}
      </span>
      <button className="icon-btn small" aria-label={t("lz.dismiss")} title={t("lz.dismiss")} onClick={() => setLaserFlow({ refusal: null })}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}

export function StartButton({ variant }: StartButtonProps) {
  const t = useT();
  const { facts, flow } = useLaserLive();
  // Re-mark region (jigs/): Start runs only inside it while one is set; the label then says so.
  useRemarkRegion();
  const gates = useSharedApprovalGates();
  const noPin = noPinMode(facts.machine);
  const ctx = startCtxOf(facts, variant, { checklistIncomplete: noPin && !gates.complete });
  const st = startState(ctx);
  const title = noPin ? t("lz.startTitle") : t("lz.requestTitle");
  const press = () => void startFlow(ctx);
  const button = (
    <button
      className={`btn btn-start btn-start-${variant}`}
      disabled={!st.enabled}
      onClick={press}
      title={st.reason ? `${st.reason}\n${title}` : title}
      // The name stays when a narrow layout hides the label (the tablet rail shows the icon only).
      aria-label={st.label}
      aria-describedby={st.reason ? `start-reason-${variant}` : undefined}
    >
      <Icon name="run" size={18} />
      <span className="btn-start-label">{st.label}</span>
    </button>
  );
  const reason = st.reason && (
    <span id={`start-reason-${variant}`} className="start-reason" role="status">
      {st.reason}
    </span>
  );
  if (variant === "toolbar") {
    return (
      <span className="start-toolbar">
        {button}
        {reason}
        <PreflightNotes variant="toolbar" />
        {flow.refusal && (
          <button className="start-refusal-pill" role="alert" title={flow.refusal} aria-label={`${t("lz.refused")}: ${flow.refusal}`} onClick={() => setLaserFlow({ refusal: null })}>
            ! {t("lz.refused")}
          </button>
        )}
      </span>
    );
  }
  return (
    <div className={`start-block start-block-${variant}`}>
      {variant === "sheet" && noPin && <ApprovalChecks state={gates} disabled={flow.starting} showDoor={false} noPin />}
      <StartRefusal text={flow.refusal} />
      <PreflightNotes variant={variant} />
      {button}
      {reason}
    </div>
  );
}
