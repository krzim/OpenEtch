// Laser window (LightBurn's Laser window, docked Fusion-style): the device dropdown, machine status, job progress with
// Pause / Resume / End after this run (components/JobControls.tsx), Frame with its options popover and Stop frame
// (components/FramingOptions.tsx), Start or Request run with the run options (Repeat, Run continuously, Start Marking
// input, Cut Selected Graphics), the job counter (Count, Reset Count, Stop At), Optimization Settings and Rotary Setup,
// and STOP, which is always allowed. With the operator PIN, Request run never fires by itself: the run waits for PIN
// approval in the Runs card. Without a PIN (`--approval click`), Start burns the job in one click like LightBurn's
// (with the Job Checklist and Require Framing shown inline when they are on), and the foot switch cycle's state is shown
// (press to frame, press again to burn what was framed).

import { useEffect, useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import type { LinkState } from "../api/useEngine.ts";
import { FramingControls } from "../components/FramingOptions.tsx";
import { MachineJog } from "../components/MachineJog.tsx";
import { JobCounter, JobProgress, RunOptionsPanel, useJobStatus } from "../components/JobControls.tsx";
import { PendingApprovals } from "../components/LaserPanel.tsx";
import { footCycle, footCycleText, noPinMode } from "../api/runs.ts";
import { ApprovalChecks, useSharedApprovalGates } from "../libs/JobSafety.tsx";
import { setLaserFlow, useLaserFacts, useLaserFlow, useLaserSource } from "../laser/laserContext.ts";
import { StepBar } from "../laser/StepBar.tsx";
import { StartButton } from "../laser/StartButton.tsx";
import { StatusChip } from "../laser/StatusChip.tsx";
import { ContextStripPortal } from "./contextStrip.tsx";
import { useLayoutMode } from "./layoutMode.ts";
import { OptimizationDialog } from "../components/OptimizationDialog.tsx";
import { RotaryDialog } from "../components/RotaryDialog.tsx";
import { Icon } from "../components/ui.tsx";
import { Dock } from "./Dock.tsx";
import { usePrefs, useT } from "./usePrefs.ts";

export interface LaserDockProps {
  machine: MachineStatus | null;
  httpError: string | null;
  link: LinkState;
  doc: GDocument | null;
  /** The device dropdown (Devices.tsx DeviceBar), when the engine has a device registry. */
  device?: ReactNode;
  /** Why this device cannot frame or run, or null. */
  designOnly: string | null;
  /** Selected object ids (Frame selection, Cut Selected Graphics). */
  selection: string[];
  pendingRuns: number;
  has: (cmd: string) => boolean;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
  /** Refetch the machine status (after a frame, a request, a pause). */
  onChanged: () => void;
  onStop: () => void;
  /** More Laser window controls (src/libs/LaserExtras.tsx: Red Dot Always On, Laser Tools, Batch, Repeat table, Job log). */
  extras?: ReactNode;
}

export function machineState(m: MachineStatus | null): { text: "offline" | "framing" | "running" | "busy" | "ready" | "noLaser"; sim: boolean } {
  if (!m) return { text: "offline", sim: false };
  const sim = !!(m.simulated ?? m.sim);
  if (m.framing) return { text: "framing", sim };
  if (m.running) return { text: "running", sim };
  if (m.busy) return { text: "busy", sim };
  return { text: m.connected ? "ready" : "noLaser", sim };
}

/** Rotary Setup asked for by a hotkey (Ctrl+Shift+R, LightBurn's default); the Laser window opens its dialog. */
let rotaryAsked = false;
const rotaryListeners = new Set<() => void>();
export function requestRotarySetup(): void {
  rotaryAsked = true;
  for (const l of rotaryListeners) l();
}

export function LaserDock(p: LaserDockProps) {
  const t = useT();
  const job = useJobStatus(p.link);
  const [dialog, setDialog] = useState<"optimization" | "rotary" | null>(null);
  const canRotaryNow = p.has("job.set_rotary");
  useEffect(() => {
    const take = () => {
      if (!rotaryAsked) return;
      rotaryAsked = false;
      if (canRotaryNow) setDialog("rotary");
    };
    take();
    rotaryListeners.add(take);
    return () => {
      rotaryListeners.delete(take);
    };
  }, [canRotaryNow]);
  const st = machineState(p.machine);
  const stateText = {
    offline: p.httpError ? t("laser.unreachable") : t("laser.loading"),
    framing: t("laser.framing"),
    running: job.job.paused ? t("job.paused") : t("laser.running"),
    busy: t("laser.busy"),
    ready: st.sim ? t("laser.readySim") : t("laser.ready"),
    noLaser: t("laser.noLaser"),
  }[st.text];
  const dotClass = st.text === "ready" || st.text === "framing" || st.text === "running" ? "ok" : st.text === "busy" ? "wait" : "bad";
  // No PIN on this engine: Start burns at once (LightBurn's Start); else Request run makes a run to approve with the PIN,
  // right below it. Both go through laser/startFlow.ts, like the toolbar, the hotkey and Start here.
  const noPin = noPinMode(p.machine);
  const gates = useSharedApprovalGates();
  const cycle = footCycle(p.machine);
  const cycleText = footCycleText(p.machine);
  const pending = p.pendingRuns > 0;
  // What the Start button, the status chip and the banner need from here (laser/laserContext.ts).
  useLaserSource("dock", {
    machine: p.machine,
    link: p.link,
    httpError: p.httpError,
    designOnly: p.designOnly,
    workspace: "laser",
    selection: p.selection,
    revision: p.doc?.revision ?? null,
    docEmpty: !!p.doc && p.doc.objects.length === 0,
    notify: p.notify,
    report: p.report,
    refreshMachine: p.onChanged,
    approvalsInDock: pending,
  });
  // Beginner Mode's step bar: a frame (from here, a hotkey or the foot switch) counts for the revision it traced.
  const flow = useLaserFlow();
  const facts = useLaserFacts();
  const revision = p.doc?.revision ?? null;
  const framingNow = !!p.machine?.framing;
  useEffect(() => {
    if (framingNow) setLaserFlow({ framedRevision: revision });
  }, [framingNow, revision]);
  // Beginner Mode hides Optimization Settings and the rotary controls, as LightBurn's does.
  const beginnerMode = usePrefs().beginnerMode;
  const canOpt = p.has("job.set_optimization") && !beginnerMode;
  const canRotary = p.has("job.set_rotary") && !beginnerMode;
  // Tablet: the status chip, Frame and Start in the context strip, reachable without opening the drawer (P5).
  const layoutMode = useLayoutMode().layout;
  const tablet = layoutMode === "tablet";
  // Phone: Start (with the Job Checklist) is pinned under the Job sheet's header (App.tsx, StartButton "sheet"), so it
  // shows at every sheet height; here Frame comes right after the status, before the step bar and the progress.
  const phone = layoutMode === "phone";
  const framing = (
    <div className="laser-buttons">
      <FramingControls
        machine={p.machine}
        doc={p.doc}
        selection={p.selection}
        designOnly={p.designOnly}
        run={p.run}
        notify={p.notify}
        report={p.report}
        onChanged={p.onChanged}
      />
    </div>
  );
  return (
    <Dock id="laser" title={t("laser.title")}>
      {tablet && (
        <ContextStripPortal>
          <div className="laser-strip" role="toolbar" aria-label={t("laser.title")}>
            <StatusChip />
            <FramingControls
              compact
              machine={p.machine}
              doc={p.doc}
              selection={p.selection}
              designOnly={p.designOnly}
              run={p.run}
              notify={p.notify}
              report={p.report}
              onChanged={p.onChanged}
            />
            <StartButton variant="toolbar" />
          </div>
        </ContextStripPortal>
      )}
      {p.device && <div className="laser-device">{p.device}</div>}
      <div className="laser-status" role="status">
        <span className={`dot ${dotClass}`} />
        <span>{stateText}</span>
        {p.pendingRuns > 0 && <span className="pill">{t("laser.pending", { count: p.pendingRuns })}</span>}
      </div>
      {p.designOnly && <p className="hint">{p.designOnly}</p>}
      {phone && framing}
      {beginnerMode && (
        <StepBar
          previewed={facts.estimateS !== null}
          framing={framingNow}
          framedRevision={flow.framedRevision}
          revision={revision}
          running={job.job.active || p.machine?.running === true}
        />
      )}

      <JobProgress state={job} notify={p.notify} report={p.report} />

      {!phone && framing}
      {!phone && noPin && <ApprovalChecks state={gates} disabled={flow.starting} showDoor={false} />}
      {!phone && (
        <div className="laser-start-row">
          <StartButton variant="dock" />
          <button className="btn btn-stop laser-stop" onClick={p.onStop} title={t("lz.stopTitle")}>
            <Icon name="stop" size={20} /> {t("laser.stop")}
          </button>
        </div>
      )}
      <PendingApprovals machine={p.machine} docRevision={p.doc?.revision ?? null} onChanged={p.onChanged} report={p.report} />

      {cycleText && (
        <p className={`foot-cycle foot-cycle-${cycle?.phase ?? "off"}${cycle?.phase === "off" ? " hint" : " run-hint"}`} role="status">
          {cycleText}
        </p>
      )}
      {/* A GRBL or Ruida gantry has no start-marking (foot pedal) input. */}
      <RunOptionsPanel selected={p.selection.length} hasStartInput={p.machine?.driver === "grbl" || p.machine?.driver === "ruida" ? false : null} />
      {p.extras}
      <JobCounter state={job} report={p.report} />
      <MachineJog designOnly={p.designOnly} notify={p.notify} report={p.report} onChanged={p.onChanged} />

      {(canOpt || canRotary) && (
        <div className="laser-buttons laser-more">
          {canOpt && (
            <button className="btn small" onClick={() => setDialog("optimization")} title={t("opt.title")}>
              {t("opt.button")}
            </button>
          )}
          {canRotary && (
            <button className="btn small" onClick={() => setDialog("rotary")} title={t("rot.title")}>
              {t("rot.button")}
            </button>
          )}
        </div>
      )}
      {dialog === "optimization" && <OptimizationDialog doc={p.doc} run={p.run} report={p.report} onClose={() => setDialog(null)} />}
      {dialog === "rotary" && <RotaryDialog doc={p.doc} machine={p.machine} run={p.run} report={p.report} notify={p.notify} onClose={() => setDialog(null)} />}
    </Dock>
  );
}
