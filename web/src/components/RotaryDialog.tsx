// LightBurn's galvo Rotary Setup, saved with the project (`job.set_rotary`): enable, chuck or roller, steps per
// rotation, object and roller diameters, split size (strip width), cylinder correction and taper (tapered tumblers).
// Shows what a run will do with the current design (circumference, strips, a design that would wrap over itself) and
// whether the active device has a rotary on its extension axis. "Test rotate" turns the rotary to the second strip
// and back (POST /api/machine/axis; motion only, never emission).

import { useEffect, useMemo, useState } from "react";
import type { Invocation } from "../api/client.ts";
import {
  DEFAULT_ROTARY,
  circumference,
  rotaryOf,
  rotaryProblems,
  setRotary,
  stepsPerObjectTurn,
  stripCount,
  type Rotary,
} from "../api/cutOptions.ts";
import { jobApi } from "../api/jobApi.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import { objectBounds, unionBoxes } from "../geometry.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { lengthText } from "../workspace/units.ts";
import { CheckField, Modal, NumberField } from "./ui.tsx";
import "./job.css";

export function RotaryDialog(p: {
  doc: GDocument | null;
  machine: MachineStatus | null;
  run: (inv: Invocation) => Promise<unknown>;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  onClose: () => void;
}) {
  const t = useT();
  const lu = usePrefs().units;
  const [r, setR] = useState<Rotary>(() => rotaryOf(p.doc));
  const [busy, setBusy] = useState(false);
  const [axisRole, setAxisRole] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    jobApi
      .machineInfo()
      .then((i) => {
        const ax = (i as { axis?: { role?: string } | null } | null)?.axis;
        if (alive) setAxisRole(ax ? (ax.role ?? "z") : null);
      })
      .catch(() => alive && setAxisRole(undefined));
    return () => {
      alive = false;
    };
  }, []);
  const set = (x: Partial<Rotary>) => setR((v) => ({ ...v, ...x }));
  const problems = rotaryProblems(r);
  const bad = Object.keys(problems).length > 0;
  const box = useMemo(() => {
    const d = p.doc;
    if (!d) return null;
    const out = new Set(d.layers.filter((l) => l.output).map((l) => l.id));
    return unionBoxes(d.objects.filter((o) => out.has(o.layer)).map(objectBounds));
  }, [p.doc]);
  const height = box ? box.y1 - box.y0 : 0;
  const circ = circumference(r);
  const wraps = height > circ + 1e-6;
  const save = () => {
    if (bad) return;
    setBusy(true);
    p.run(setRotary(r))
      .then(p.onClose)
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  const test = async () => {
    const steps = Math.round((r.split_width_mm / circ) * stepsPerObjectTurn(r));
    setBusy(true);
    try {
      await jobApi.axis({ steps: Math.max(1, Math.min(65535, steps)) });
      await new Promise((res) => window.setTimeout(res, 1200));
      await jobApi.axis({ steps: 0 });
      p.notify(t("rot.tested", { steps }));
    } catch (e) {
      p.report(e);
    } finally {
      setBusy(false);
    }
  };
  const err = (k: string) => (problems as Record<string, string | undefined>)[k];
  return (
    <Modal
      title={t("rot.title")}
      onClose={p.onClose}
      onSubmit={save}
      footer={
        <>
          <button className="btn" disabled={busy || axisRole !== "rotary" || bad} title={axisRole !== "rotary" ? t("rot.noAxis") : t("rot.testTitle")} onClick={() => void test()}>
            {t("rot.test")}
          </button>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || bad} onClick={save}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      <CheckField label={t("rot.enabled")} checked={r.enabled} onChange={(enabled) => set({ enabled })} />
      {axisRole === null && <p className="warn-text">{t("rot.noAxisWarn")}</p>}
      {axisRole === "z" && <p className="warn-text">{t("rot.zAxisWarn")}</p>}
      <div className="segmented rot-kind" role="radiogroup" aria-label={t("rot.kind")}>
        <button role="radio" aria-checked={r.kind === "chuck"} className={r.kind === "chuck" ? "active" : ""} onClick={() => set({ kind: "chuck" })}>
          {t("rot.chuck")}
        </button>
        <button role="radio" aria-checked={r.kind === "roller"} className={r.kind === "roller" ? "active" : ""} onClick={() => set({ kind: "roller" })}>
          {t("rot.roller")}
        </button>
      </div>
      <NumberField label={t("rot.steps")} value={r.steps_per_rotation} min={1} max={1_000_000} digits={0} onCommit={(v) => v !== null && set({ steps_per_rotation: Math.round(v) })} />
      {err("steps_per_rotation") && <p className="form-error">{err("steps_per_rotation")}</p>}
      <NumberField label={t("rot.objectDia")} unit="mm" value={r.object_diameter_mm} min={0.1} max={10000} onCommit={(v) => v !== null && set({ object_diameter_mm: v })} />
      {err("object_diameter_mm") && <p className="form-error">{err("object_diameter_mm")}</p>}
      {r.kind === "roller" && (
        <>
          <NumberField label={t("rot.rollerDia")} unit="mm" value={r.roller_diameter_mm} min={0.1} max={10000} onCommit={(v) => v !== null && set({ roller_diameter_mm: v })} />
          {err("roller_diameter_mm") && <p className="form-error">{err("roller_diameter_mm")}</p>}
        </>
      )}
      <NumberField label={t("rot.split")} unit="mm" value={r.split_width_mm} min={0.1} max={10000} onCommit={(v) => v !== null && set({ split_width_mm: v })} />
      {err("split_width_mm") && <p className="form-error">{err("split_width_mm")}</p>}
      <p className="hint">{t("rot.splitHint")}</p>
      <CheckField label={t("rot.cylinder")} checked={r.cylinder_correction} onChange={(cylinder_correction) => set({ cylinder_correction })} />
      <CheckField
        label={t("rot.taper")}
        checked={r.taper !== null}
        onChange={(on) => set({ taper: on ? { end_diameter_mm: r.object_diameter_mm, length_mm: Math.max(10, Math.round(box ? box.x1 - box.x0 : 100)) } : null })}
      />
      {r.taper && (
        <div className="job-indent">
          <NumberField label={t("rot.endDia")} unit="mm" value={r.taper.end_diameter_mm} min={0.1} max={10000} onCommit={(v) => v !== null && r.taper && set({ taper: { ...r.taper, end_diameter_mm: v } })} />
          <NumberField label={t("rot.length")} unit="mm" value={r.taper.length_mm} min={0.1} max={10000} onCommit={(v) => v !== null && r.taper && set({ taper: { ...r.taper, length_mm: v } })} />
          <p className="hint">{t("rot.taperHint")}</p>
        </div>
      )}
      <dl className="kv rot-facts">
        <dt>{t("rot.circumference")}</dt>
        <dd>{lengthText(circ, lu, 2)}</dd>
        <dt>{t("rot.stepsPerTurn")}</dt>
        <dd>{Number(stepsPerObjectTurn(r).toFixed(1))}</dd>
        {height > 0 && (
          <>
            <dt>{t("rot.design")}</dt>
            <dd>
              {lengthText(height, lu, 2)} · {t("rot.strips", { count: stripCount(r, height) })}
            </dd>
          </>
        )}
      </dl>
      {wraps && <p className="form-error">{t("rot.wraps")}</p>}
      <p className="hint">{t("rot.hint")}</p>
      <button className="btn small" onClick={() => setR({ ...DEFAULT_ROTARY, enabled: r.enabled })}>
        {t("opt.defaults")}
      </button>
    </Modal>
  );
}
