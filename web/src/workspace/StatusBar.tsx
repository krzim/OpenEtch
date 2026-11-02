// Status bar along the bottom of the window (LightBurn's): pointer X/Y (the snapped point while a snap applies, with
// its label), the selection's size, the snap settings, zoom, and the display unit (click to switch mm / in).

import { useMemo, useSyncExternalStore } from "react";
import type { GDocument } from "../api/types.ts";
import { objectBounds, unionBoxes } from "../geometry.ts";
import type { Selection } from "../selection.ts";
import { updatePrefs } from "./prefs.ts";
import { coord, formatLength, zoomPercent } from "./units.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { getViewStatus, sendViewCommand, subscribeViewStatus } from "./viewStatus.ts";
import "./workspace.css";

export interface StatusBarProps {
  doc: GDocument | null;
  selection: Selection;
  snap: { objects: boolean; grid: boolean; gridMm: number };
  /** Short machine state, e.g. "Ready (simulator)". */
  machineText?: string;
}

export function StatusBar(p: StatusBarProps) {
  const t = useT();
  const prefs = usePrefs();
  const vs = useSyncExternalStore(subscribeViewStatus, getViewStatus, getViewStatus);
  const u = prefs.units;
  const box = useMemo(() => {
    if (!p.doc || p.selection.objects.length === 0) return null;
    const set = new Set(p.selection.objects);
    return unionBoxes(p.doc.objects.filter((o) => set.has(o.id)).map(objectBounds));
  }, [p.doc, p.selection.objects]);
  const at = vs.snap?.p ?? vs.cursor;
  const snapText = [p.snap.objects ? t("status.snapObjects") : "", p.snap.grid ? t("status.snapGrid", { size: formatLength(p.snap.gridMm, u, 3) }) : ""]
    .filter(Boolean)
    .join(" + ");
  return (
    <footer className="statusbar" aria-label={t("status.label")}>
      <span className={`sb-cell sb-xy${vs.snap ? " snapped" : ""}`} title={vs.snap ? t("status.snappedTitle") : t("status.cursorTitle")}>
        {at ? (
          <>
            X <b>{coord(at[0], u)}</b> Y <b>{coord(at[1], u)}</b>
          </>
        ) : (
          <span className="muted">X — Y —</span>
        )}
      </span>
      {vs.snap && vs.snap.label && <span className="sb-cell sb-snap-label">{vs.snap.label}</span>}
      <span className="sb-cell" title={t("status.selectionTitle")}>
        {box ? (
          <>
            {t("status.selection", { count: p.selection.objects.length })} <b>{formatLength(box.x1 - box.x0, u)}</b> × <b>{formatLength(box.y1 - box.y0, u)}</b>
          </>
        ) : (
          <span className="muted">{t("status.noSelection")}</span>
        )}
      </span>
      <span className="sb-spacer" />
      {p.machineText && <span className="sb-cell sb-machine">{p.machineText}</span>}
      <span className="sb-cell sb-snap" title={t("status.snapTitle")}>
        {t("status.snap")}: {snapText || t("status.off")}
      </span>
      <button className="sb-cell sb-btn" title={t("status.zoomTitle")} onClick={() => sendViewCommand("fit")}>
        {Math.round(zoomPercent(vs.scale))}%
      </button>
      <button className="sb-cell sb-btn" title={t("status.unitsTitle")} onClick={() => updatePrefs({ units: u === "mm" ? "in" : "mm" })}>
        {u === "mm" ? "mm" : "in"} · {prefs.speedUnit}
      </button>
    </footer>
  );
}
