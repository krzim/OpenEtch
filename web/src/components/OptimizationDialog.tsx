// LightBurn's Optimization Settings (the Cut Planner), saved with the project: enable re-ordering, the Order By list
// (layer, group, priority; reorderable), cut inner shapes first, direction order, reduce travel, reduce direction
// changes, best starting point (and corners), best direction, remove overlapping lines (with the distance) and sort
// cuts last. OK sends one `job.set_optimization` with the changed keys (one undo step).

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import { DEFAULT_OPTIMIZATION, moveOrder, optimizationOf, optimizationPatch, setOptimization, type CutDirection, type Optimization, type OrderBy } from "../api/cutOptions.ts";
import type { GDocument } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { CheckField, Icon, Modal, NumberField } from "./ui.tsx";
import "./job.css";

export function OptimizationDialog(p: { doc: GDocument | null; run: (inv: Invocation) => Promise<unknown>; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const cur = optimizationOf(p.doc);
  const [o, setO] = useState<Optimization>(cur);
  const [busy, setBusy] = useState(false);
  const set = (x: Partial<Optimization>) => setO((v) => ({ ...v, ...x }));
  const orderLabel: Record<OrderBy, string> = { layer: t("opt.byLayer"), group: t("opt.byGroup"), priority: t("opt.byPriority") };
  const missing = (["layer", "group", "priority"] as OrderBy[]).filter((x) => !o.order_by.includes(x));
  const dirs: { v: CutDirection | ""; label: string }[] = [
    { v: "", label: t("opt.dirNone") },
    { v: "top_to_bottom", label: t("opt.dirTop") },
    { v: "bottom_to_top", label: t("opt.dirBottom") },
    { v: "left_to_right", label: t("opt.dirLeft") },
    { v: "right_to_left", label: t("opt.dirRight") },
  ];
  const save = () => {
    const patch = optimizationPatch(cur, o);
    if (!patch) return p.onClose();
    setBusy(true);
    p.run(setOptimization(patch))
      .then(p.onClose)
      .catch(p.report)
      .finally(() => setBusy(false));
  };
  const off = !o.enabled;
  return (
    <Modal
      title={t("opt.title")}
      onClose={p.onClose}
      onSubmit={save}
      footer={
        <>
          <button className="btn" onClick={() => setO(DEFAULT_OPTIMIZATION)} title={t("opt.defaultsTitle")}>
            {t("opt.defaults")}
          </button>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={save}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      <CheckField label={t("opt.enabled")} checked={o.enabled} onChange={(enabled) => set({ enabled })} />
      <p className="hint">{t("opt.enabledHint")}</p>
      <fieldset className="job-fieldset" disabled={off}>
        <legend>{t("opt.orderBy")}</legend>
        <ol className="opt-order">
          {o.order_by.map((x, i) => (
            <li key={x}>
              <span className="opt-order-name">{orderLabel[x]}</span>
              <button className="icon-btn small" aria-label={t("opt.up")} disabled={i === 0} onClick={() => set({ order_by: moveOrder(o.order_by, i, -1) })}>
                <Icon name="chevronUp" size={14} />
              </button>
              <button className="icon-btn small" aria-label={t("opt.down")} disabled={i === o.order_by.length - 1} onClick={() => set({ order_by: moveOrder(o.order_by, i, 1) })}>
                <Icon name="chevronDown" size={14} />
              </button>
              <button className="icon-btn small" aria-label={t("opt.remove")} disabled={o.order_by.length === 1} onClick={() => set({ order_by: o.order_by.filter((y) => y !== x) })}>
                <Icon name="close" size={14} />
              </button>
            </li>
          ))}
        </ol>
        {missing.length > 0 && (
          <div className="et-dlg-row">
            {missing.map((x) => (
              <button key={x} className="btn small" onClick={() => set({ order_by: [...o.order_by, x] })}>
                <Icon name="plus" size={14} /> {orderLabel[x]}
              </button>
            ))}
          </div>
        )}
        <p className="hint">{t("opt.orderHint")}</p>
      </fieldset>
      <fieldset className="job-fieldset" disabled={off}>
        <legend>{t("opt.planner")}</legend>
        <CheckField label={t("opt.innerFirst")} checked={o.inner_first} onChange={(inner_first) => set({ inner_first })} />
        <label className="field">
          <span className="field-label">{t("opt.direction")}</span>
          <span className="field-input">
            <select value={o.direction_order ?? ""} onChange={(e) => set({ direction_order: (e.target.value || null) as CutDirection | null })}>
              {dirs.map((d) => (
                <option key={d.v} value={d.v}>
                  {d.label}
                </option>
              ))}
            </select>
          </span>
        </label>
        <CheckField label={t("opt.reduceTravel")} checked={o.reduce_travel} onChange={(reduce_travel) => set({ reduce_travel })} />
        <CheckField label={t("opt.reduceDir")} checked={o.reduce_direction_changes} onChange={(reduce_direction_changes) => set({ reduce_direction_changes })} />
        <CheckField label={t("opt.bestStart")} checked={o.best_start} onChange={(best_start) => set({ best_start })} />
        <div className="job-indent">
          <CheckField label={t("opt.corners")} checked={o.corners} onChange={(corners) => set({ corners })} />
        </div>
        <CheckField label={t("opt.bestDir")} checked={o.best_direction} onChange={(best_direction) => set({ best_direction })} />
        <CheckField label={t("opt.removeOverlaps")} checked={o.remove_overlaps} onChange={(remove_overlaps) => set({ remove_overlaps })} />
        {o.remove_overlaps && (
          <NumberField
            label={t("opt.overlapDist")}
            unit="mm"
            value={o.overlap_distance_mm}
            min={0}
            max={10}
            digits={4}
            onCommit={(v) => v !== null && set({ overlap_distance_mm: v })}
          />
        )}
        <CheckField label={t("opt.cutsLast")} checked={o.cuts_last} onChange={(cuts_last) => set({ cuts_last })} />
      </fieldset>
      <p className="hint">{t("opt.savedHint")}</p>
    </Modal>
  );
}
