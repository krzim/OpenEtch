// The Fit part of a text's properties (OpenEtch's text auto-fit, short-list #10; xTool Studio's Auto Fit): fit the
// text into a box or a circle, growing or shrinking it (Fit), filling the box exactly (Stretch) or only shrinking what
// does not fit (Shrink only), and "Fit to stock", which centres the text on the stock and fits it into 80 % of it (a
// coin's face). Each change is one text.update (Fit to stock one batch); the canvas draws the area dashed
// (useTextTools.tsx).

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { NumberField } from "../components/ui.tsx";
import { objectBounds } from "../geometry.ts";
import { useT } from "../workspace/usePrefs.ts";
import { fitFields, fitToStock, startSize, type FitMode, type FitShape } from "./fitModel.ts";
import { textUpdate, type TextObject } from "./textModel.ts";
import { HelpLink } from "../guide/HelpLink.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export function FitSection(p: { o: TextObject; doc: GDocument; run: Run; report: (e: unknown) => void; onWarnings: (r: unknown) => void }) {
  const t = useT();
  const g = p.o.geometry;
  const f = g.fit ?? null;
  const shape: FitShape = f?.shape ?? "none";
  const mode: FitMode = f?.mode ?? "contain";
  const onPath = !!g.path;
  // The sizes a box or circle starts from: the text's current bounds, so choosing Box changes nothing visible at once.
  const [start] = useState(() => {
    const b = objectBounds(p.o);
    return startSize(b ? { w: b.x1 - b.x0, h: b.y1 - b.y0 } : null);
  });
  const send = (s: FitShape, w: number | null, h: number | null, m: FitMode) => {
    const fit = fitFields(s, w, h, m);
    if (!fit) return;
    void p.run(textUpdate(p.o.id, { fit })).then(p.onWarnings).catch(p.report);
  };
  const width = f?.width_mm ?? (shape === "circle" ? Math.max(start.w, start.h) : start.w);
  const height = f?.shape === "box" ? f.height_mm : start.h;
  return (
    <>
      <h4>{t("text.fitHead")} <HelpLink to="text#fit-text-into-a-box-or-circle" topic={t("text.fitHead")} /></h4>
      {onPath ? (
        <p className="hint">{t("text.fitOnPath")}</p>
      ) : (
        <>
          <div className="tx-seg tx-fit-shape" role="radiogroup" aria-label={t("text.fitShape")}>
            {(["none", "box", "circle"] as const).map((s) => (
              <button
                key={s}
                type="button"
                role="radio"
                aria-checked={shape === s}
                className={shape === s ? "on" : ""}
                onClick={() => {
                  if (s === shape) return;
                  if (s === "none") send("none", null, null, mode);
                  else if (s === "box") send("box", f?.width_mm ?? start.w, f?.shape === "box" ? f.height_mm : start.h, mode === "stretch" ? "contain" : mode);
                  else send("circle", f ? Math.max(f.width_mm, f.height_mm) : Math.max(start.w, start.h), null, mode === "stretch" ? "contain" : mode);
                }}
              >
                {t(s === "none" ? "text.fitNone" : s === "box" ? "text.fitBox" : "text.fitCircle")}
              </button>
            ))}
          </div>
          {shape !== "none" && (
            <>
              {shape === "box" ? (
                <div className="field-grid">
                  <NumberField label={t("text.fitWidth")} unit="mm" value={width} min={0.01} max={10000} digits={2} onCommit={(v) => v !== null && v > 0 && send("box", v, height, mode)} />
                  <NumberField label={t("text.fitHeight")} unit="mm" value={height} min={0.01} max={10000} digits={2} onCommit={(v) => v !== null && v > 0 && send("box", width, v, mode)} />
                </div>
              ) : (
                <NumberField label={t("text.fitDiameter")} unit="mm" value={width} min={0.01} max={10000} digits={2} onCommit={(v) => v !== null && v > 0 && send("circle", v, null, mode)} />
              )}
              <label className="field">
                <span className="field-label">{t("text.fitMode")}</span>
                <span className="field-input">
                  <select value={mode} onChange={(e) => send(shape, width, height, e.target.value as FitMode)}>
                    <option value="contain">{t("text.fitContain")}</option>
                    {shape === "box" && <option value="stretch">{t("text.fitStretch")}</option>}
                    <option value="shrink">{t("text.fitShrink")}</option>
                  </select>
                </span>
              </label>
              <p className="hint">{t(shape === "circle" ? "text.fitCircleHint" : "text.fitBoxHint")}</p>
            </>
          )}
          {p.doc.stock && (
            <div className="tx-row-btns">
              <button className="btn" title={t("text.fitStockTitle")} onClick={() => void p.run(fitToStock(p.o, p.doc.stock!, mode)).then(p.onWarnings).catch(p.report)}>
                {t("text.fitStock")}
              </button>
            </div>
          )}
        </>
      )}
    </>
  );
}
