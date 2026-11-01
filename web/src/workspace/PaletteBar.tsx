// LightBurn's colour palette along the bottom of the canvas: C00..C29 and the tool layers T1/T2. Clicking a swatch
// moves the selection to that colour's layer, creating the layer first when there is none (two commands then:
// `layer.add`, `object.set_layer`; a tool layer is also set to no output, as LightBurn's tool layers never burn).
// Swatches whose layer exists are marked; the selection's layer is outlined.

import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import type { GDocument } from "../api/types.ts";
import { assignPlan, layerForEntry, PALETTE, type PaletteEntry } from "./palette.ts";
import { useT } from "./usePrefs.ts";
import "./workspace.css";

export interface PaletteBarProps {
  doc: GDocument | null;
  selected: string[];
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  /** After a click: the layer the selection is now on (to show it in the properties). */
  onLayer?: (layer: string) => void;
}

export function PaletteBar(p: PaletteBarProps) {
  const t = useT();
  const { doc } = p;
  if (!doc) return null;
  const selLayers = new Set(doc.objects.filter((o) => p.selected.includes(o.id)).map((o) => o.layer));

  const click = async (e: PaletteEntry) => {
    const plan = assignPlan(doc.layers, doc.objects, p.selected, e);
    try {
      if (plan.kind === "none") {
        if (plan.reason === "no-selection") p.notify(t("palette.selectFirst"), "error");
        return;
      }
      let layer = plan.kind === "existing" ? plan.layer : "";
      if (plan.kind === "create") {
        const r = await p.run(C.layerAdd(plan.name, plan.color));
        layer = createdIds(r)[0] ?? (r && typeof r === "object" ? String((r as { id?: unknown }).id ?? "") : "");
        if (!layer) return;
        if (!plan.output) await p.run(C.layerUpdate(layer, { output: false }));
      }
      await p.run(C.setLayer(p.selected, layer));
      p.onLayer?.(layer);
    } catch (err) {
      p.report(err);
    }
  };

  return (
    <div className="palette-bar" role="toolbar" aria-label={t("palette.label")}>
      {PALETTE.map((e) => {
        const layer = layerForEntry(doc.layers, e);
        const current = !!layer && selLayers.has(layer.id);
        return (
          <button
            key={e.index}
            className={`pal-swatch${layer ? " used" : ""}${current ? " current" : ""}${e.tool ? " tool" : ""}`}
            style={{ background: e.color }}
            title={
              layer
                ? t("palette.titleUsed", { name: e.name, layer: layer.name })
                : e.tool
                  ? t("palette.titleTool", { name: e.name })
                  : t("palette.titleNew", { name: e.name })
            }
            aria-label={e.name}
            onClick={() => void click(e)}
          >
            {e.tool ? <span className="pal-text">{e.name}</span> : null}
          </button>
        );
      })}
    </div>
  );
}
