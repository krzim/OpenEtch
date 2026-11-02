// The tab settings of one pass (LightBurn's Tabs / Bridges in the Cut Settings Editor), shown under the Tabs switch
// in CutOptions.tsx: generation (automatic / manual), tab size, tabs per shape or spacing with Limit Max Tabs, Skip
// Inner Shapes and Tab Cut Power. Every change sends the whole `tabs` object (`layer.update settings.options.tabs`).

import type { TabSettings } from "../api/cutOptions.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";

export function TabFields({ tabs, onChange }: { tabs: TabSettings; onChange: (t: TabSettings) => void }) {
  const t = useT();
  const set = (patch: Partial<TabSettings>) => onChange({ ...tabs, ...patch });
  const num = (f: (v: number) => void) => (v: number | null) => v !== null && f(v);
  const auto = tabs.generation === "auto";
  return (
    <div className="job-indent">
      <label className="field">
        <span className="field-label">{t("tabs.generation")}</span>
        <span className="field-input">
          <select
            value={tabs.generation}
            title={t("tabs.genTitle")}
            onChange={(e) => {
              const generation = e.target.value === "manual" ? "manual" : "auto";
              // Automatic tabs need a count or a spacing.
              set(generation === "auto" && tabs.count < 1 && tabs.spacing_mm <= 0 ? { generation, count: 4 } : { generation });
            }}
          >
            <option value="auto">{t("tabs.genAuto")}</option>
            <option value="manual">{t("tabs.genManual")}</option>
          </select>
        </span>
      </label>
      <NumberField label={t("cut.tabSize")} unit="mm" value={tabs.size_mm} min={0.001} digits={3} onCommit={num((size_mm) => set({ size_mm }))} />
      {auto && (
        <>
          <NumberField
            label={t("cut.tabCount")}
            value={tabs.spacing_mm > 0 ? null : tabs.count}
            min={1}
            max={1000}
            digits={0}
            placeholder={t("cut.bySpacing")}
            onCommit={num((v) => set({ count: Math.round(v), spacing_mm: 0 }))}
          />
          <NumberField
            label={t("cut.tabSpacing")}
            unit="mm"
            value={tabs.spacing_mm > 0 ? tabs.spacing_mm : null}
            min={0}
            digits={3}
            allowEmpty
            placeholder={t("cut.byCount")}
            onCommit={(v) => set({ spacing_mm: v ?? 0, count: v ? tabs.count : Math.max(1, tabs.count || 4) })}
          />
          {tabs.spacing_mm > 0 && (
            <NumberField
              label={t("tabs.maxTabs")}
              value={tabs.max_tabs > 0 ? tabs.max_tabs : null}
              min={0}
              max={1000}
              digits={0}
              allowEmpty
              placeholder={t("tabs.noLimit")}
              onCommit={(v) => set({ max_tabs: v ? Math.round(v) : 0 })}
            />
          )}
          <CheckField label={t("tabs.skipInner")} checked={tabs.skip_inner} onChange={(skip_inner) => set({ skip_inner })} />
        </>
      )}
      <NumberField label={t("tabs.cutPower")} unit="%" value={tabs.cut_power_pct} min={0} max={100} digits={1} onCommit={num((cut_power_pct) => set({ cut_power_pct }))} />
      {tabs.cut_power_pct > 0 && <p className="hint">{t("tabs.cutPowerHint")}</p>}
      {!auto && <p className="hint">{t("tabs.manualHint")}</p>}
    </div>
  );
}
