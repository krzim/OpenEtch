// Numeric edits toolbar (LightBurn's Numeric Edits, above the canvas): X, Y, W, H and rotation of the selection's
// bounding box at the chosen 9-point anchor (shared with the Properties panel), lock aspect, and mm or % sizing. Each field commits one
// `object.transform` on Enter or blur. Values show in the display unit (Preferences); fields accept maths ("20/3").
// The fields come in groups (anchor + X/Y, size, rotation, units) that wrap onto a second row and get narrower in a
// narrow canvas column, so nothing is clipped or needs sideways scrolling.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import type { GDocument } from "../api/types.ts";
import { objectBounds, transformParts, unionBoxes } from "../geometry.ts";
import { NumberField } from "../components/ui.tsx";
import type { Selection } from "../selection.ts";
import { ANCHORS, anchorPoint, moveTo, resizeTo, rotationDelta, scalePercent, useAnchor } from "./numeric.ts";
import { updatePrefs } from "./prefs.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import "./workspace.css";

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
        <NumberField label="X" unit="mm" value={box ? ax : null} disabled={none} onCommit={(v) => setXY(v, null)} />
        <NumberField label="Y" unit="mm" value={box ? ay : null} disabled={none} onCommit={(v) => setXY(null, v)} />
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
          <LockIcon locked={lock} />
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
        <NumberField label={single || none ? t("numeric.rotate") : t("numeric.rotateBy")} unit="°" value={box ? (single ? rot : 0) : null} digits={2} disabled={none} onCommit={setRot} />
      </div>
      <span className="numeric-sep" />
      <div className="segmented unit-toggle" role="radiogroup" aria-label={t("prefs.units")} title={t("numeric.unitsTitle")}>
        {(["mm", "in"] as const).map((u) => (
          <button key={u} role="radio" aria-checked={prefs.units === u} className={prefs.units === u ? "active" : ""} onClick={() => updatePrefs({ units: u })}>
            {u}
          </button>
        ))}
      </div>
    </div>
  );
}

function LockIcon({ locked }: { locked: boolean }) {
  return (
    <svg className="icon" width={18} height={18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={locked ? "M6 11h12v9H6z M8.5 11V8a3.5 3.5 0 017 0v3" : "M6 11h12v9H6z M8.5 11V8a3.5 3.5 0 016.8-1.2"} />
    </svg>
  );
}
