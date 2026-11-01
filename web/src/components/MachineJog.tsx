// Small machine moves from the Laser window, none of which can fire the laser: the red dot held at a field point
// (red-dot jog, POST /api/machine/pointer; Stop frame ends it) and the Z lift on the board's extension axis
// (POST /api/machine/axis, UI only; shown only when the active device has a Z axis, from GET /api/machine/info).

import { useEffect, useState } from "react";
import { onEngineEvent } from "../api/events.ts";
import { jobApi } from "../api/jobApi.ts";
import { useT } from "../workspace/usePrefs.ts";
import { NumberField, PairField } from "./ui.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

interface ZAxis {
  min_mm: number;
  max_mm: number;
}

/** The active device's Z lift, or null (no axis, a rotary, or an older engine). Refetched when the device changes. */
function useZAxis(): ZAxis | null {
  const [ax, setAx] = useState<ZAxis | null>(null);
  const [seq, setSeq] = useState(0);
  useEffect(() => onEngineEvent((ev) => (ev.type === "device_changed" || ev.type === "devices_changed") && setSeq((n) => n + 1)), []);
  useEffect(() => {
    let alive = true;
    jobApi
      .machineInfo()
      .then((i) => {
        const a = (i as { axis?: { role?: string; min_mm?: number; max_mm?: number } | null } | null)?.axis;
        if (alive) setAx(a && (a.role ?? "z") === "z" && typeof a.min_mm === "number" && typeof a.max_mm === "number" ? { min_mm: a.min_mm, max_mm: a.max_mm } : null);
      })
      .catch(() => alive && setAx(null));
    return () => {
      alive = false;
    };
  }, [seq]);
  return ax;
}

export function MachineJog(p: { designOnly: string | null; notify: Notify; report: (e: unknown) => void; onChanged: () => void }) {
  const t = useT();
  const [pt, setPt] = useState<[number, number]>([0, 0]);
  const z = useZAxis();
  const [zmm, setZmm] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const act = (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true);
    fn()
      .then(() => p.notify(ok))
      .catch(p.report)
      .finally(() => {
        setBusy(false);
        p.onChanged();
      });
  };
  return (
    <details className="cut-opts machine-jog">
      <summary>{t("jog.title")}</summary>
      <PairField labels={[t("jog.x"), t("jog.y")]} unit="mm" value={pt} onCommit={setPt} />
      <button className="btn small" disabled={busy || !!p.designOnly} title={t("jog.pointTitle")} onClick={() => act(() => jobApi.pointer(pt[0], pt[1]), t("jog.pointed"))}>
        {t("jog.point")}
      </button>
      {z && (
        <>
          <NumberField label={t("jog.z")} unit="mm" value={zmm} min={z.min_mm} max={z.max_mm} digits={3} placeholder={`${z.min_mm}…${z.max_mm}`} onCommit={setZmm} />
          <button className="btn small" disabled={busy || zmm === null} title={t("jog.zTitle")} onClick={() => zmm !== null && act(() => jobApi.axis({ z_mm: zmm }), t("jog.zMoved", { z: zmm }))}>
            {t("jog.zMove")}
          </button>
        </>
      )}
      <p className="hint">{t("jog.hint")}</p>
    </details>
  );
}
