// OpenEtch Design's Preview window: what the full app's Laser window becomes in the browser-only build
// (docs/design-only/plan.md "Show instead"). There is no laser, so no framing, Start, approvals, job progress, red dot,
// rotary or production: the design device picker (the field the design is laid out, previewed and timed for), the job's
// time estimate labelled "Estimate for <device>", and Optimization Settings (they change the order and the time). App
// shows it in the Preview workspace with Cuts / Layers and the Toolpath card under it (and in the phone's Job sheet).

import { useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import { formatDuration } from "../api/laser.ts";
import type { GDocument } from "../api/types.ts";
import { OptimizationDialog } from "../components/OptimizationDialog.tsx";
import { Dock } from "../workspace/Dock.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import "./designmode.css";

export function DesignPreviewDock(p: {
  /** The design device dropdown (components/Devices.tsx DeviceBar). */
  device?: ReactNode;
  /** The active design device's name, for "Estimate for <device>". */
  deviceName: string | null;
  /** The job's estimated time, s (planned in the browser), or null while there is none. */
  estimateS: number | null;
  doc: GDocument | null;
  has: (cmd: string) => boolean;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
}) {
  const t = useT();
  const [opt, setOpt] = useState(false);
  // Beginner Mode hides Optimization Settings, as the Laser window does.
  const canOpt = p.has("job.set_optimization") && !usePrefs().beginnerMode;
  const device = p.deviceName ?? t("dm.designDevice");
  return (
    <Dock help="preview-and-framing#time-estimates" id="laser" title={t("dm.preview")}>
      <div className="dm-section">
        <div className="dm-label">{t("dm.designDevice")}</div>
        {p.device && <div className="laser-device">{p.device}</div>}
        <p className="hint">{t("dm.designDeviceHint")}</p>
      </div>
      <p className="dm-estimate" role="status" title={t("dm.estimateForTitle", { device })}>
        {p.estimateS !== null && p.estimateS > 0 ? t("dm.estimateFor", { device, time: formatDuration(p.estimateS) }) : t("dm.estimateNone")}
      </p>
      {canOpt && (
        <div className="laser-buttons laser-more">
          <button className="btn small" onClick={() => setOpt(true)} title={t("opt.title")}>
            {t("opt.button")}
          </button>
        </div>
      )}
      {opt && <OptimizationDialog doc={p.doc} run={p.run} report={p.report} onClose={() => setOpt(false)} />}
    </Dock>
  );
}
