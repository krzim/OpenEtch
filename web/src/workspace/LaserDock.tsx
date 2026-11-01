// Laser window (LightBurn's Laser window, docked Fusion-style): the device dropdown, machine status, Frame with the
// options the engine has (bounding box or outline, whole job or selection only), Stop frame, Request run (it never
// fires by itself: the run waits for PIN approval in the Runs card), Pause/Resume and job progress shown but disabled
// until the engine supports them, and STOP, which is always allowed.

import { useState, type ReactNode } from "react";
import type { MachineStatus } from "../api/types.ts";
import { Icon } from "../components/ui.tsx";
import { Dock } from "./Dock.tsx";
import { useT } from "./usePrefs.ts";

export type FrameMode = "bounds" | "outline";

export interface LaserDockProps {
  machine: MachineStatus | null;
  httpError: string | null;
  /** The device dropdown (Devices.tsx DeviceBar), when the engine has a device registry. */
  device?: ReactNode;
  /** Why this device cannot frame or run, or null. */
  designOnly: string | null;
  selected: number;
  pendingRuns: number;
  onFrame: (o: { mode: FrameMode; selection: boolean }) => void;
  onFrameStop: () => void;
  onRequestRun: () => void;
  onStop: () => void;
}

export function machineState(m: MachineStatus | null): { text: "offline" | "framing" | "running" | "busy" | "ready" | "noLaser"; sim: boolean } {
  if (!m) return { text: "offline", sim: false };
  const sim = !!(m.simulated ?? m.sim);
  if (m.framing) return { text: "framing", sim };
  if (m.running) return { text: "running", sim };
  if (m.busy) return { text: "busy", sim };
  return { text: m.connected ? "ready" : "noLaser", sim };
}

export function LaserDock(p: LaserDockProps) {
  const t = useT();
  const [mode, setMode] = useState<FrameMode>("bounds");
  const [selOnly, setSelOnly] = useState(false);
  const st = machineState(p.machine);
  const stateText = {
    offline: p.httpError ? t("laser.unreachable") : t("laser.loading"),
    framing: t("laser.framing"),
    running: t("laser.running"),
    busy: t("laser.busy"),
    ready: st.sim ? t("laser.readySim") : t("laser.ready"),
    noLaser: t("laser.noLaser"),
  }[st.text];
  const dotClass = st.text === "ready" || st.text === "framing" || st.text === "running" ? "ok" : st.text === "busy" ? "wait" : "bad";
  const notYet = t("laser.notYet");
  // The engine says why framing is refused (no laser, a run streaming); the button only knows the device kind.
  const canFrame = !p.designOnly;
  return (
    <Dock id="laser" title={t("laser.title")}>
      {p.device && <div className="laser-device">{p.device}</div>}
      <div className="laser-status" role="status">
        <span className={`dot ${dotClass}`} />
        <span>{stateText}</span>
        {p.pendingRuns > 0 && <span className="pill">{t("laser.pending", { count: p.pendingRuns })}</span>}
      </div>
      {p.designOnly && <p className="hint">{p.designOnly}</p>}

      <div className="laser-frame">
        <div className="segmented" role="radiogroup" aria-label={t("laser.frameMode")}>
          <button role="radio" aria-checked={mode === "bounds"} className={mode === "bounds" ? "active" : ""} onClick={() => setMode("bounds")} title={t("laser.boundsTitle")}>
            {t("laser.bounds")}
          </button>
          <button role="radio" aria-checked={mode === "outline"} className={mode === "outline" ? "active" : ""} onClick={() => setMode("outline")} title={t("laser.outlineTitle")}>
            {t("laser.outline")}
          </button>
        </div>
        <label className="laser-check" title={t("laser.selectionTitle")}>
          <input type="checkbox" checked={selOnly} onChange={(e) => setSelOnly(e.target.checked)} />
          {t("laser.selectionOnly")}
        </label>
      </div>
      <div className="laser-buttons">
        <button
          className="btn"
          disabled={!canFrame || (selOnly && p.selected === 0)}
          title={p.designOnly ?? (selOnly && p.selected === 0 ? t("laser.selectFirst") : t("laser.frameTitle"))}
          onClick={() => p.onFrame({ mode, selection: selOnly })}
        >
          <Icon name="frame" size={18} /> {t("laser.frame")}
        </button>
        <button className="btn" onClick={p.onFrameStop} title={t("laser.frameStopTitle")}>
          <Icon name="frameStop" size={18} /> {t("laser.frameStop")}
        </button>
        <button className="btn btn-primary" disabled={!!p.designOnly} onClick={p.onRequestRun} title={p.designOnly ?? t("laser.requestTitle")}>
          <Icon name="run" size={18} /> {t("laser.request")}
        </button>
      </div>
      <div className="laser-buttons">
        {/* Not in the engine yet: kept visible (LightBurn's layout) and explained. */}
        <span title={notYet}>
          <button className="btn" disabled aria-label={t("laser.pause")}>
            <Icon name="pause" size={18} /> {t("laser.pause")}
          </button>
        </span>
        <span title={notYet}>
          <button className="btn" disabled aria-label={t("laser.resume")}>
            <Icon name="play" size={18} /> {t("laser.resume")}
          </button>
        </span>
        <button className="btn btn-stop laser-stop" onClick={p.onStop} title={t("laser.stopTitle")}>
          <Icon name="stop" size={18} /> {t("laser.stop")}
        </button>
      </div>
      <div className="laser-progress" title={notYet}>
        <span className="laser-progress-label">{t("laser.progress")}</span>
        <progress max={100} value={0} aria-label={t("laser.progress")} />
        <span className="muted">{st.text === "running" ? t("laser.runningNoProgress") : "—"}</span>
      </div>
    </Dock>
  );
}
