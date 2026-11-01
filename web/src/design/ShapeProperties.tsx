// LightBurn's Shape Properties for a lone primitive: a rectangle's width, height and corner radius, an ellipse's
// radii, a polygon's sides, radius, star and corner radius. Each change is one `shape.update` (the centre, rotation
// and layer stay; crates/commands/src/builtin/design.rs). Shown under Properties in the Design workspace.

import { useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import type { Selection } from "../selection.ts";
import { DCMD, shapeUpdate } from "./designApi.ts";
import { shapeParams, shapePatchProblem, type ShapeParams } from "./model.ts";

export function ShapeProperties(p: {
  doc: GDocument | null;
  selection: Selection;
  has: (cmd: string) => boolean;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
}) {
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
      setErr(e instanceof Error ? e.message : String(e));
      p.report(e);
    });
  };
  const n = (v: number | null, f: (x: number) => void) => v !== null && f(v);
  return (
    <section className="prop-section dz-shape-props">
      <h3>Shape properties</h3>
      {sp.kind === "rect" && (
        <>
          <div className="field-grid">
            <NumberField label="Width" unit="mm" value={sp.width_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ width_mm: x }))} />
            <NumberField label="Height" unit="mm" value={sp.height_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ height_mm: x }))} />
          </div>
          <NumberField
            label="Corner radius"
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
          <NumberField label="Radius X" unit="mm" value={sp.rx_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ rx_mm: x }))} />
          <NumberField label="Radius Y" unit="mm" value={sp.ry_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ ry_mm: x }))} />
        </div>
      )}
      {sp.kind === "polygon" && (
        <>
          <div className="field-grid">
            <NumberField label={sp.star_ratio ? "Points" : "Sides"} value={sp.sides} min={3} max={1000} digits={0} onCommit={(v) => n(v, (x) => send({ sides: Math.round(x) }))} />
            <NumberField label="Radius" unit="mm" value={sp.radius_mm} min={0.001} onCommit={(v) => n(v, (x) => send({ radius_mm: x }))} />
          </div>
          <CheckField
            label="Star"
            checked={sp.star_ratio !== null}
            onChange={(on) => {
              if (!on && sp.star_ratio !== null) setLastRatio(sp.star_ratio);
              send({ star_ratio: on ? lastRatio : null });
            }}
          />
          {sp.star_ratio !== null && (
            <NumberField
              label="Inner radius"
              unit="%"
              value={Math.round(sp.star_ratio * 1000) / 10}
              min={1}
              max={99}
              digits={1}
              onCommit={(v) => n(v, (x) => send({ star_ratio: Math.round(x * 10) / 1000 }))}
            />
          )}
          <NumberField label="Corner radius" unit="mm" value={sp.corner_radius_mm} min={0} onCommit={(v) => n(v, (x) => send({ corner_radius_mm: x }))} />
        </>
      )}
      {err && <p className="form-error">{err}</p>}
      <p className="hint">Edits the {sp.kind === "rect" ? "rectangle" : sp.kind === "ellipse" ? "ellipse" : sp.star_ratio ? "star" : "polygon"} itself; its centre, rotation and layer stay. Convert to path to edit nodes.</p>
    </section>
  );
}
