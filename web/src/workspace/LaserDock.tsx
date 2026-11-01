// Laser window (LightBurn's Laser window, docked Fusion-style): the device dropdown, machine status, job progress with
// Pause / Resume / End after this run (components/JobControls.tsx), Frame with its options popover and Stop frame
// (components/FramingOptions.tsx), Request run with the run options (Repeat, Run continuously, Start Marking input,
// Cut Selected Graphics), the job counter (Count, Reset Count, Stop At), Optimization Settings and Rotary Setup, and
// STOP, which is always allowed. Request run never fires by itself: the run waits for PIN approval in the Runs card.

import { useEffect, useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import type { LinkState } from "../api/useEngine.ts";
import { FramingControls } from "../components/FramingOptions.tsx";
import { MachineJog } from "../components/MachineJog.tsx";
import { JobCounter, JobProgress, RunOptionsPanel, requestRunWithOptions, useJobStatus } from "../components/JobControls.tsx";
import { OptimizationDialog } from "../components/OptimizationDialog.tsx";
import { RotaryDialog } from "../components/RotaryDialog.tsx";
import { Icon } from "../components/ui.tsx";
import { Dock } from "./Dock.tsx";
import { useT } from "./usePrefs.ts";

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
  const [requesting, setRequesting] = useState(false);
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
  const request = () => {
    setRequesting(true);
    requestRunWithOptions(p.selection)
      .then((msg) => p.notify(msg))
      .catch(p.report)
      .finally(() => {
        setRequesting(false);
        p.onChanged();
      });
  };
  const canOpt = p.has("job.set_optimization");
  const canRotary = p.has("job.set_rotary");
  return (
    <Dock id="laser" title={t("laser.title")}>
      {p.device && <div className="laser-device">{p.device}</div>}
      <div className="laser-status" role="status">
        <span className={`dot ${dotClass}`} />
        <span>{stateText}</span>
        {p.pendingRuns > 0 && <span className="pill">{t("laser.pending", { count: p.pendingRuns })}</span>}
      </div>
      {p.designOnly && <p className="hint">{p.designOnly}</p>}

      <JobProgress state={job} notify={p.notify} report={p.report} />

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
      <div className="laser-buttons">
        <button className="btn btn-primary" disabled={!!p.designOnly || requesting} onClick={request} title={p.designOnly ?? t("laser.requestTitle")}>
          <Icon name="run" size={18} /> {t("laser.request")}
        </button>
        <button className="btn btn-stop laser-stop" onClick={p.onStop} title={t("laser.stopTitle")}>
          <Icon name="stop" size={18} /> {t("laser.stop")}
        </button>
      </div>

      <RunOptionsPanel selected={p.selection.length} hasStartInput={null} />
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
