// Numeric edits toolbar (LightBurn's Numeric Edits, above the canvas): X, Y, W, H and rotation of the selection's
// bounding box at the chosen 9-point anchor (shared with the Properties panel), lock aspect, and mm or % sizing. Each field commits one
// `object.transform` on Enter or blur. Values show in the display unit (Preferences); fields accept maths ("20/3").
// It is the selection state of the context strip (docs/ux/ui-plan-2026-11.md 1.1): one row that never wraps; the fields
// get narrower in a narrow canvas column and the row scrolls sideways as a last resort. The mm / in switch lives in the
// status bar only.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument } from "../api/types.ts";
import { objectBounds, transformParts, unionBoxes } from "../geometry.ts";
import { NumberField } from "../components/ui.tsx";
import type { Selection } from "../selection.ts";
import { ANCHORS, anchorPoint, moveTo, resizeTo, rotationDelta, scalePercent, useAnchor } from "./numeric.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import "./workspace.css";
import { Icon } from "../components/ui.tsx";

export interface NumericBarProps {
  doc: GDocument | null;
  selection: Selection;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
}

export function NumericBar(p: NumericBarProps) {
  const t = useT();
  const prefs = usePrefs();
  const [anchor, setAnchor] = useAnchor();
  const [lock, setLock] = useState(true);
  const [percent, setPercent] = useState(false);
  const objs = p.doc ? p.doc.objects.filter((o) => p.selection.objects.includes(o.id)) : [];
  const box = unionBoxes(objs.map(objectBounds));
  const ids = objs.map((o) => o.id);
  const single = objs.length === 1 ? objs[0] : null;
  const rot = single ? transformParts(single.transform).rotationDeg : 0;
  const none = !box;
  const exec = (inv: Invocation) => void p.run(inv).catch(p.report);
  const [ax, ay] = box ? anchorPoint(box, anchor) : [0, 0];
  const w = box ? box.x1 - box.x0 : 0;
  const h = box ? box.y1 - box.y0 : 0;

  const setXY = (x: number | null, y: number | null) => {
    if (!box) return;
    const d = moveTo(box, anchor, x ?? ax, y ?? ay);
    if (d) exec(C.transform(ids, { translate: d }));
  };
  const setSize = (nw: number | null, nh: number | null) => {
    if (!box) return;
    const r = percent ? scalePercent(box, anchor, nw, nh, lock) : resizeTo(box, anchor, nw, nh, lock);
    if (r) exec(C.transform(ids, { scale: r.scale, about: r.about }));
  };
  const setRot = (v: number | null) => {
    if (!box || v === null) return;
    const d = rotationDelta(rot, v, !!single);
    if (d !== null) exec(C.transform(ids, { rotate_deg: d, about: [Math.round(ax * 1000) / 1000, Math.round(ay * 1000) / 1000] }));
  };

  // In % mode the fields always start at 100 (relative to now); a remount per revision puts them back after an edit.
  const sizeUnit = percent ? "%" : "mm";
  return (
    <div className={`numeric-bar${none ? " empty" : ""}`} role="toolbar" aria-label={t("numeric.label")}>
      <div className="num-group">
        <div className="anchor-grid small" role="radiogroup" aria-label={t("numeric.anchor")}>
          {ANCHORS.map((a) => (
            <button key={a} role="radio" aria-checked={a === anchor} className={a === anchor ? "active" : ""} title={t("numeric.anchorTitle")} onClick={() => setAnchor(a)}>
              <span className="anchor-dot" />
            </button>
          ))}
        </div>
        <NumberField label="X" unit="mm" signed value={box ? ax : null} disabled={none} onCommit={(v) => setXY(v, null)} />
        <NumberField label="Y" unit="mm" signed value={box ? ay : null} disabled={none} onCommit={(v) => setXY(null, v)} />
      </div>
      <div className="num-group">
        <NumberField key={percent ? `w%${p.doc?.revision}` : "w"} label="W" unit={sizeUnit} value={box ? (percent ? 100 : w) : null} min={percent ? 0.1 : 0.001} disabled={none} onCommit={(v) => setSize(v, null)} />
        <button
          className={`icon-btn small lock-btn${lock ? " active" : ""}`}
          aria-pressed={lock}
          title={lock ? t("numeric.lockOn") : t("numeric.lockOff")}
          aria-label={t("numeric.lock")}
          onClick={() => setLock((v) => !v)}
        >
          <Icon name={lock ? "lock" : "unlock"} size={18} />
        </button>
        <NumberField key={percent ? `h%${p.doc?.revision}` : "h"} label="H" unit={sizeUnit} value={box ? (percent ? 100 : h) : null} min={percent ? 0.1 : 0.001} disabled={none} onCommit={(v) => setSize(null, v)} />
        <div className="segmented unit-toggle" role="radiogroup" aria-label={t("numeric.sizeMode")}>
          <button role="radio" aria-checked={!percent} className={!percent ? "active" : ""} onClick={() => setPercent(false)} title={t("numeric.sizeAbs")}>
            {prefs.units}
          </button>
          <button role="radio" aria-checked={percent} className={percent ? "active" : ""} onClick={() => setPercent(true)} title={t("numeric.sizePct")}>
            %
          </button>
        </div>
      </div>
      <div className="num-group">
        <NumberField label={single || none ? t("numeric.rotate") : t("numeric.rotateBy")} unit="°" signed value={box ? (single ? rot : 0) : null} digits={2} disabled={none} onCommit={setRot} />
      </div>
    </div>
  );
}

