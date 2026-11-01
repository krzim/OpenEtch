// Preferences (LightBurn's File > Settings, the display side): theme, language, UI scale, units, grid, snapping,
// nudge steps, mouse wheel and the crosshair cursor. Changes apply at once and are kept in this browser; the engine
// and the document are never touched (and stay in mm). Plus Beginner Mode (LightBurn's Editor Settings) and the
// engine's Project locations (files/ProjectFolders.tsx, saved by the engine).


import { NumberField } from "../components/ui.tsx";
import { ProjectLocationsSection } from "../files/ProjectFolders.tsx";
import { LightBurnPrefsImport } from "../files/LightBurnPrefsImport.tsx";
import { libsToast } from "../libs/store.ts";
import { languages, type MsgKey } from "./i18n.ts";
import { DEFAULT_NUDGE, SNAP_TOGGLES, resetPrefs, updatePrefs, type LengthUnit, type SpeedUnit, type ThemePref } from "./prefs.ts";
import { usePrefs, useT } from "./usePrefs.ts";
import { WsModal } from "./WsModal.tsx";

export function PreferencesDialog(p: { onClose: () => void; onHotkeys: () => void }) {
  const t = useT();
  const prefs = usePrefs();
  const seg = <V extends string>(value: V, options: { v: V; label: string }[], set: (v: V) => void, aria: string) => (
    <div className="segmented" role="radiogroup" aria-label={aria}>
      {options.map((o) => (
        <button key={o.v} role="radio" aria-checked={value === o.v} className={value === o.v ? "active" : ""} onClick={() => set(o.v)}>
          {o.label}
        </button>
      ))}
    </div>
  );
  const check = (label: string, checked: boolean, set: (v: boolean) => void, hint?: string) => (
    <label className="pref-check" title={hint}>
      <input type="checkbox" checked={checked} onChange={(e) => set(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
  return (
    <WsModal
      title={t("prefs.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={() => resetPrefs(["hotkeys", "docks"])} title={t("prefs.resetTitle")}>
            {t("prefs.reset")}
          </button>
          <LightBurnPrefsImport notify={libsToast} />
          <span className="tool-spacer" />
          <button className="btn btn-primary" autoFocus onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      <div className="prefs-grid">
        <section className="prefs-section">
          <h4>{t("prefs.appearance")}</h4>
          <div className="pref-row">
            <span className="pref-label">{t("prefs.theme")}</span>
            {seg<ThemePref>(
              prefs.theme,
              [
                { v: "light", label: t("theme.light") },
                { v: "dark", label: t("theme.dark") },
                { v: "system", label: t("theme.system") },
              ],
              (theme) => updatePrefs({ theme }),
              t("prefs.theme"),
            )}
          </div>
          <label className="pref-row">
            <span className="pref-label">{t("prefs.language")}</span>
            <span className="field-input">
              <select value={prefs.language} onChange={(e) => updatePrefs({ language: e.target.value })}>
                {languages().map((l) => (
                  <option key={l.code} value={l.code}>
                    {l.name}
                  </option>
                ))}
              </select>
            </span>
          </label>
          <label className="pref-row">
            <span className="pref-label">{t("prefs.uiScale")}</span>
            <input
              type="range"
              min={0.8}
              max={1.5}
              step={0.05}
              value={prefs.uiScale}
              onChange={(e) => updatePrefs({ uiScale: Number(e.target.value) })}
              aria-valuetext={`${Math.round(prefs.uiScale * 100)}%`}
            />
            <span className="pref-value">{Math.round(prefs.uiScale * 100)}%</span>
          </label>
          <label className="pref-row">
            <span className="pref-label">{t("prefs.iconScale")}</span>
            <input
              type="range"
              min={0.75}
              max={2}
              step={0.05}
              value={prefs.iconScale}
              onChange={(e) => updatePrefs({ iconScale: Number(e.target.value) })}
              aria-valuetext={`${Math.round(prefs.iconScale * 100)}%`}
            />
            <span className="pref-value">{Math.round(prefs.iconScale * 100)}%</span>
          </label>
          <label className="pref-row">
            <span className="pref-label">{t("prefs.handleScale")}</span>
            <input
              type="range"
              min={0.75}
              max={3}
              step={0.05}
              value={prefs.handleScale}
              onChange={(e) => updatePrefs({ handleScale: Number(e.target.value) })}
              aria-valuetext={`${Math.round(prefs.handleScale * 100)}%`}
            />
            <span className="pref-value">{Math.round(prefs.handleScale * 100)}%</span>
          </label>
          {check(t("prefs.crosshair"), prefs.crosshair, (crosshair) => updatePrefs({ crosshair }))}
        </section>

        <section className="prefs-section">
          <h4>{t("prefs.unitsSection")}</h4>
          <div className="pref-row">
            <span className="pref-label">{t("prefs.units")}</span>
            {seg<LengthUnit>(
              prefs.units,
              [
                { v: "mm", label: t("prefs.mm") },
                { v: "in", label: t("prefs.inch") },
              ],
              (units) => updatePrefs({ units }),
              t("prefs.units"),
            )}
          </div>
          <div className="pref-row">
            <span className="pref-label">{t("prefs.speedUnit")}</span>
            {seg<SpeedUnit>(
              prefs.speedUnit,
              [
                { v: "mm/s", label: "mm/s" },
                { v: "mm/min", label: "mm/min" },
                { v: "in/s", label: "in/s" },
              ],
              (speedUnit) => updatePrefs({ speedUnit }),
              t("prefs.speedUnit"),
            )}
          </div>
          <p className="hint">{t("prefs.unitsHint")}</p>
        </section>

        <section className="prefs-section">
          <h4>{t("prefs.grid")}</h4>
          {check(t("prefs.gridVisible"), prefs.gridVisible, (gridVisible) => updatePrefs({ gridVisible }))}
          <NumberField
            label={t("prefs.gridSpacing")}
            unit="mm"
            value={prefs.gridMm || null}
            allowEmpty
            placeholder={t("prefs.auto")}
            min={0.01}
            max={1000}
            onCommit={(v) => updatePrefs({ gridMm: v ?? 0 })}
          />
          <p className="hint">{t("prefs.gridHint")}</p>
        </section>

        <section className="prefs-section">
          <h4>{t("prefs.snapping")}</h4>
          <NumberField label={t("prefs.snapTol")} unit="px" value={prefs.snapTolPx} min={1} max={40} digits={0} onCommit={(v) => v !== null && updatePrefs({ snapTolPx: Math.round(v) })} />
          <div className="pref-checks">
            {SNAP_TOGGLES.map((k) => (
              <div key={k}>{check(t(`snap.${k}` as MsgKey), prefs.snapKinds[k], (on) => updatePrefs((cur) => ({ snapKinds: { ...cur.snapKinds, [k]: on } })))}</div>
            ))}
          </div>
          <p className="hint">{t("prefs.snapHint")}</p>
        </section>

        <section className="prefs-section">
          <h4>{t("prefs.editing")}</h4>
          <div className="pref-fields">
            <NumberField label={t("prefs.nudge")} unit="mm" value={prefs.nudge.normal} min={0.001} onCommit={(v) => v !== null && updatePrefs((c) => ({ nudge: { ...c.nudge, normal: v } }))} />
            <NumberField label={t("prefs.nudgeShift")} unit="mm" value={prefs.nudge.shift} min={0.001} onCommit={(v) => v !== null && updatePrefs((c) => ({ nudge: { ...c.nudge, shift: v } }))} />
            <NumberField label={t("prefs.nudgeCtrl")} unit="mm" value={prefs.nudge.ctrl} min={0.001} onCommit={(v) => v !== null && updatePrefs((c) => ({ nudge: { ...c.nudge, ctrl: v } }))} />
            <NumberField
              label={t("prefs.nudgeCtrlShift")}
              unit="mm"
              value={prefs.nudge.ctrlShift}
              min={0.001}
              onCommit={(v) => v !== null && updatePrefs((c) => ({ nudge: { ...c.nudge, ctrlShift: v } }))}
            />
          </div>
          <button className="link-btn" onClick={() => updatePrefs({ nudge: DEFAULT_NUDGE })}>
            {t("prefs.nudgeReset")}
          </button>
          {check(t("prefs.invertWheel"), prefs.invertWheel, (invertWheel) => updatePrefs({ invertWheel }))}
          <button className="btn" onClick={p.onHotkeys}>
            {t("prefs.editHotkeys")}
          </button>
          {check(t("prefs.beginnerMode"), prefs.beginnerMode, (beginnerMode) => updatePrefs({ beginnerMode }), t("prefs.beginnerModeHint"))}
          <p className="hint">{t("prefs.beginnerModeHint")}</p>
        </section>

        <ProjectLocationsSection />
      </div>
    </WsModal>
  );
}
