// LightBurn's Shape Properties for a lone primitive: a rectangle's width, height and corner radius, an ellipse's
// radii, a polygon's sides, radius, star and corner radius. Each change is one `shape.update` (the centre, rotation
// and layer stay; crates/commands/src/builtin/design.rs). Shown under Properties in the Design workspace.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { Selection } from "../selection.ts";
import { DCMD, shapeUpdate } from "./designApi.ts";
import { shapeParams, shapePatchProblem, type ShapeParams } from "./model.ts";
import { errorText } from "../api/errorText.ts";
import { HelpLink } from "../guide/HelpLink.tsx";

export function ShapeProperties(p: {
  doc: GDocument | null;
  selection: Selection;
  has: (cmd: string) => boolean;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
}) {
  const t = useT();
  const [err, setErr] = useState<string | null>(null);
  const [lastRatio, setLastRatio] = useState(0.5);
  if (!p.doc || p.selection.objects.length !== 1 || !p.has(DCMD.shapeUpdate)) return null;
  const o = p.doc.objects.find((x) => x.id === p.selection.objects[0]);
  const sp: ShapeParams | null = o ? shapeParams(o.geometry) : null;
  if (!o || !sp) return null;
  const send = (patch: Record<string, number | null>) => {
    const problem = shapePatchProblem(sp, patch);
    if (problem) {
      setErr(problem);
      return;
    }
    setErr(null);
    void p.run(shapeUpdate(o.id, patch)).catch((e: unknown) => {
      setErr(errorText(e));
      p.report(e);
    });
  };
  const n = (v: number | null, f: (x: number) => void) => v !== null && f(v);
  return (
    <section className="prop-section dz-shape-props">
      <h3>{t("dz.shapeProps")} <HelpLink to="drawing-shapes#shape-properties" topic={t("dz.shapeProps")} /></h3>
      {sp.kind === "rect" && (
        <>
          <div className="field-grid">
            <NumberField label={t("dz.width")} unit="mm" value={sp.width_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ width_mm: x }))} />
            <NumberField label={t("dz.height")} unit="mm" value={sp.height_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ height_mm: x }))} />
          </div>
          <NumberField
            label={t("dz.cornerRadius")}
            unit="mm"
            value={sp.corner_radius_mm}
            min={0}
            max={Math.min(sp.width_mm, sp.height_mm) / 2}
            onCommit={(v) => n(v, (x) => send({ corner_radius_mm: x }))}
          />
        </>
      )}
      {sp.kind === "ellipse" && (
        <div className="field-grid">
          <NumberField label={t("dz.radiusX")} unit="mm" value={sp.rx_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ rx_mm: x }))} />
          <NumberField label={t("dz.radiusY")} unit="mm" value={sp.ry_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ ry_mm: x }))} />
        </div>
      )}
      {sp.kind === "polygon" && (
        <>
          <div className="field-grid">
            <NumberField label={sp.star_ratio ? t("dz.points") : t("dz.sides")} value={sp.sides} min={3} max={1000} digits={0} onCommit={(v) => n(v, (x) => send({ sides: Math.round(x) }))} />
            {sp.ry_mm === null ? (
              <NumberField label={t("dz.radius")} unit="mm" value={sp.radius_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ radius_mm: x }))} />
            ) : (
              <>
                {/* A LightBurn polygon with separate radii (Rx / Ry). */}
                <NumberField label={t("dz.radiusX")} unit="mm" value={sp.radius_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ radius_mm: x }))} />
                <NumberField label={t("dz.radiusY")} unit="mm" value={sp.ry_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ ry_mm: x }))} />
              </>
            )}
          </div>
          <CheckField
            label={t("dz.star")}
            checked={sp.star_ratio !== null}
            onChange={(on) => {
              if (!on && sp.star_ratio !== null) setLastRatio(sp.star_ratio);
              send({ star_ratio: on ? lastRatio : null });
            }}
          />
          {sp.star_ratio !== null && (
            <NumberField
              label={t("dz.innerRadius")}
              unit="%"
              value={Math.round(sp.star_ratio * 1000) / 10}
              min={1}
              max={99}
              digits={1}
              onCommit={(v) => n(v, (x) => send({ star_ratio: Math.round(x * 10) / 1000 }))}
            />
          )}
          <NumberField label={t("dz.cornerRadius")} unit="mm" value={sp.corner_radius_mm} min={0} onCommit={(v) => n(v, (x) => send({ corner_radius_mm: x }))} />
        </>
      )}
      {err && <p className="form-error">{err}</p>}
      <p className="hint">{t(sp.kind === "rect" ? "dz.shapeHint.rect" : sp.kind === "ellipse" ? "dz.shapeHint.ellipse" : sp.star_ratio ? "dz.shapeHint.star" : "dz.shapeHint.polygon")}</p>
    </section>
  );
}
