// The material library as a palette (short list #6): every preset with a result colour as a swatch (its photo when
// it has one), grouped by the surface it was made for, with the lot and date on each (MOPA colour drifts between
// lots). A click picks the preset like the tree does; a double-click assigns it to the layer.

import { useMemo, useState } from "react";
import type { LibEntry, LibMaterial } from "../libs/model.ts";
import { treeEntries } from "../libs/model.ts";
import type { LongPressHandlers } from "../touch/useLongPress.ts";
import { useT } from "../workspace/usePrefs.ts";
import { FinishSwatch } from "./FinishDialog.tsx";
import { SURFACES, bySurface, paletteOf, type Surface } from "./model.ts";
import "./materials.css";

export function PalettePanel(p: {
  library: string;
  tree: readonly LibMaterial[];
  picked: string | null;
  onPick: (id: string) => void;
  onAssign: (e: LibEntry) => void;
  press?: LongPressHandlers;
}) {
  const t = useT();
  const [only, setOnly] = useState<Surface | "all">("all");
  const swatches = useMemo(() => paletteOf(p.tree), [p.tree]);
  const entries = useMemo(() => new Map(treeEntries(p.tree).map((e) => [e.id, e])), [p.tree]);
  const groups = bySurface(swatches, only);
  if (swatches.length === 0) return <p className="hint mt-palette-empty">{t("mat.paletteEmpty")}</p>;
  const present = new Set(swatches.map((s) => s.surface));
  return (
    <div className="mt-palette">
      <div className="mt-chips" role="radiogroup" aria-label={t("mat.surface")}>
        {(["all", ...SURFACES] as const)
          .filter((s) => s === "all" || present.has(s))
          .map((s) => (
            <button key={s} type="button" role="radio" aria-checked={only === s} className={`mt-chip${only === s ? " on" : ""}`} onClick={() => setOnly(s)}>
              {s === "all" ? t("mat.paletteAll") : t(`mat.surface.${s}`)}
            </button>
          ))}
      </div>
      {groups.map((g) => (
        <section key={g.surface ?? "none"} className="mt-palette-group">
          <h4>{g.surface ? t(`mat.surface.${g.surface}`) : t("mat.paletteNone")}</h4>
          <div className="mt-grid">
            {g.swatches.map((s) => {
              const e = entries.get(s.id);
              if (!e) return null;
              const lot = s.lot || s.date ? `\n${t("mat.lotDate", { lot: s.lot || "–", date: s.date || "–" })}` : "";
              return (
                <button
                  key={s.id}
                  type="button"
                  data-id={s.id}
                  className={`mt-cell${p.picked === s.id ? " selected" : ""}`}
                  aria-pressed={p.picked === s.id}
                  title={t("mat.swatchTitle", { path: s.path, color: s.color, lot })}
                  {...p.press}
                  onClick={() => p.onPick(s.id)}
                  onDoubleClick={() => p.onAssign(e)}
                >
                  <FinishSwatch library={p.library} entry={e} />
                  <span className="mt-cell-name">{s.description}</span>
                  <span className="mt-cell-meta">{[s.material, s.lot, s.date].filter(Boolean).join(" · ")}</span>
                  {s.photoStale && <span className="mt-stale" aria-hidden="true" />}
                </button>
              );
            })}
          </div>
        </section>
      ))}
    </div>
  );
}
