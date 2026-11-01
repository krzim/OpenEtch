// The rest of LightBurn's galvo cut settings for one pass (main settings or a sub-layer), under the laser parameters
// in the Properties panel: global passes (layer), the pass's own output toggle, fill options (lines per inch, fill
// grouping, flood fill, auto-rotate, dot width adjust), line options (dot mode, wobble, perforation, tabs), pause
// before / after, Z offset / Z step, and "Override timings" prefilled from the device. Each field sends one
// `layer.update` / `layer.sub_update` with a partial `settings.options` (a merge patch), `settings.timing`,
// `settings.hatch.lines_per_inch` or `global_passes`.
//
// `ShapeCutProperties` ("Shape cut settings", named apart from the geometry panel "Shape properties" of design/) is
// LightBurn's Shape Properties for the selection: Power Scale and cut order priority
// (`object.set_cut_properties`).

import { useEffect, useState } from "react";
import { api, type Invocation } from "../api/client.ts";
import {
  OPTION_DEFAULTS,
  TIMING_KEYS,
  activeOptionCount,
  deviceTiming,
  globalPasses,
  globalPassesUpdate,
  intervalToLpi,
  layerTiming,
  lpiPatch,
  optionsPatch,
  resolveOptions,
  setCutProperties,
  sharedOptions,
  timingPatch,
  type FillGrouping,
  type GalvoTiming,
  type PassRef,
} from "../api/cutOptions.ts";
import { onEngineEvent } from "../api/events.ts";
import type { CutSettings, GDocument, Layer } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { CheckField, NumberField } from "./ui.tsx";
import "./job.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

// ---- the active device's timing, for "Override timings" -------------------------------------------------------------

let timingCache: Promise<GalvoTiming | null> | null = null;

/** The active device's timing defaults (GET /api/devices, then the active device's profile), cached until it changes. */
function loadDeviceTiming(): Promise<GalvoTiming | null> {
  timingCache ??= (async () => {
    const list = (await api.devices()) as { active?: unknown };
    if (typeof list?.active !== "string") return null;
    const d = (await api.device(list.active)) as { profile?: { timing?: unknown } | null };
    return d?.profile?.timing ? deviceTiming(d.profile.timing) : null;
  })().catch(() => null);
  return timingCache;
}

onEngineEvent((ev) => {
  if (ev.type === "device_changed" || ev.type === "devices_changed") timingCache = null;
});

function useDeviceTiming(): GalvoTiming | null {
  const [t, setT] = useState<GalvoTiming | null>(null);
  useEffect(() => {
    let alive = true;
    void loadDeviceTiming().then((x) => alive && setT(x));
    return () => {
      alive = false;
    };
  }, []);
  return t;
}

const TIMING_LABEL: Record<keyof GalvoTiming, [string, string]> = {
  laser_on_delay_us: ["Laser On TC", "µs"],
  laser_off_delay_us: ["Laser Off TC", "µs"],
  polygon_delay_us: ["Polygon TC", "µs"],
  end_delay_us: ["End TC", "µs"],
  jump_speed_mm_s: ["Jump speed", "mm/s"],
  min_jump_delay_us: ["Min jump delay", "µs"],
  max_jump_delay_us: ["Max jump delay", "µs"],
  jump_distance_limit_mm: ["Jump distance limit", "mm"],
};

// ---- per-pass options -----------------------------------------------------------------------------------------------

export function CutOptionsSection(p: { layer: Layer; sub: number | null; settings: CutSettings; run: Run; report: (e: unknown) => void; has?: (cmd: string) => boolean }) {
  const t = useT();
  const ref: PassRef = { layer: p.layer.id, sub: p.sub };
  const r = resolveOptions(p.settings);
  const exec = (inv: Invocation) => void p.run(inv).catch(p.report);
  const opt = (patch: Record<string, unknown>) => exec(optionsPatch(ref, patch));
  const hatching = p.settings.mode !== "line";
  const lines = p.settings.mode !== "fill";
  const timing = layerTiming(p.settings);
  const devTiming = useDeviceTiming();
  const count = activeOptionCount(r) + (timing ? 1 : 0) + (p.sub === null && globalPasses(p.layer) > 1 ? 1 : 0);
  const num = (f: (v: number) => void) => (v: number | null) => v !== null && f(v);
  return (
    <details className="cut-opts" open={count > 0 || undefined}>
      <summary>
        {t("cut.more")}
        {count > 0 && <span className="pill">{count}</span>}
      </summary>
      {p.sub === null && (
        <NumberField label={t("cut.globalPasses")} value={globalPasses(p.layer)} min={1} max={1000} digits={0} onCommit={num((v) => exec(globalPassesUpdate(p.layer.id, v)))} />
      )}
      <CheckField label={p.sub === null ? t("cut.outputMain") : t("cut.outputSub")} checked={r.output} onChange={(output) => opt({ output })} />

      {hatching && (
        <fieldset className="job-fieldset">
          <legend>{t("cut.fill")}</legend>
          <NumberField
            label={t("cut.lpi")}
            unit="LPI"
            value={Number(intervalToLpi(p.settings.hatch.interval_mm).toFixed(2))}
            min={1}
            max={25400}
            digits={2}
            onCommit={num((v) => exec(lpiPatch(ref, v)))}
          />
          <label className="field">
            <span className="field-label">{t("cut.fillGrouping")}</span>
            <span className="field-input">
              <select value={r.fill_grouping} onChange={(e) => opt({ fill_grouping: e.target.value as FillGrouping })} title={t("cut.fillGroupingTitle")}>
                <option value="individually">{t("cut.fgIndividually")}</option>
                <option value="all_at_once">{t("cut.fgAll")}</option>
                <option value="by_group">{t("cut.fgGroup")}</option>
              </select>
            </span>
          </label>
          <CheckField label={t("cut.floodFill")} checked={r.flood_fill} onChange={(flood_fill) => opt({ flood_fill })} />
          <CheckField label={t("cut.autoRotate")} checked={r.auto_rotate} onChange={(auto_rotate) => opt({ auto_rotate })} />
          {r.auto_rotate && <p className="hint">{t("cut.autoRotateHint")}</p>}
          <NumberField label={t("cut.dotWidth")} unit="mm" value={r.fill_dot_width_mm} min={0} max={10} digits={4} onCommit={num((fill_dot_width_mm) => opt({ fill_dot_width_mm }))} />
        </fieldset>
      )}

      {lines && (
        <fieldset className="job-fieldset">
          <legend>{t("cut.line")}</legend>
          <CheckField label={t("cut.dotMode")} checked={r.dot_mode !== null} onChange={(on) => opt({ dot_mode: on ? OPTION_DEFAULTS.dot_mode : null })} />
          {r.dot_mode && (
            <div className="job-indent">
              <NumberField label={t("cut.dotTime")} unit="ms" value={r.dot_mode.dot_time_ms} min={0.01} max={1000} digits={3} onCommit={num((v) => opt({ dot_mode: { ...r.dot_mode, dot_time_ms: v } }))} />
              <NumberField label={t("cut.dotSpacing")} unit="mm" value={r.dot_mode.dot_spacing_mm} min={0.001} max={1000} digits={4} onCommit={num((v) => opt({ dot_mode: { ...r.dot_mode, dot_spacing_mm: v } }))} />
            </div>
          )}
          <CheckField label={t("cut.wobble")} checked={r.wobble !== null} onChange={(on) => opt({ wobble: on ? OPTION_DEFAULTS.wobble : null })} />
          {r.wobble && (
            <div className="job-indent">
              <NumberField label={t("cut.wobbleStep")} unit="mm" value={r.wobble.step_mm} min={0.001} max={100} digits={4} onCommit={num((v) => opt({ wobble: { ...r.wobble, step_mm: v } }))} />
              <NumberField label={t("cut.wobbleSize")} unit="mm" value={r.wobble.size_mm} min={0.001} max={100} digits={4} onCommit={num((v) => opt({ wobble: { ...r.wobble, size_mm: v } }))} />
            </div>
          )}
          <CheckField label={t("cut.perforation")} checked={r.perforation !== null} onChange={(on) => opt({ perforation: on ? OPTION_DEFAULTS.perforation : null })} />
          {r.perforation && (
            <div className="job-indent">
              <NumberField label={t("cut.perfCut")} unit="mm" value={r.perforation.cut_mm} min={0.001} digits={3} onCommit={num((v) => opt({ perforation: { ...r.perforation, cut_mm: v } }))} />
              <NumberField label={t("cut.perfSkip")} unit="mm" value={r.perforation.skip_mm} min={0.001} digits={3} onCommit={num((v) => opt({ perforation: { ...r.perforation, skip_mm: v } }))} />
            </div>
          )}
          <CheckField label={t("cut.tabs")} checked={r.tabs !== null} onChange={(on) => opt({ tabs: on ? OPTION_DEFAULTS.tabs : null })} />
          {r.tabs && (
            <div className="job-indent">
              <NumberField label={t("cut.tabSize")} unit="mm" value={r.tabs.size_mm} min={0.001} digits={3} onCommit={num((v) => opt({ tabs: { ...r.tabs, size_mm: v } }))} />
              <NumberField
                label={t("cut.tabCount")}
                value={r.tabs.spacing_mm > 0 ? null : r.tabs.count}
                min={1}
                max={1000}
                digits={0}
                placeholder={t("cut.bySpacing")}
                onCommit={num((v) => opt({ tabs: { ...r.tabs, count: Math.round(v), spacing_mm: 0 } }))}
              />
              <NumberField
                label={t("cut.tabSpacing")}
                unit="mm"
                value={r.tabs.spacing_mm > 0 ? r.tabs.spacing_mm : null}
                min={0}
                digits={3}
                allowEmpty
                placeholder={t("cut.byCount")}
                onCommit={(v) => opt({ tabs: { ...r.tabs, spacing_mm: v ?? 0, count: v ? (r.tabs?.count ?? 0) : Math.max(1, r.tabs?.count || 4) } })}
              />
            </div>
          )}
          {(r.dot_mode || r.wobble || r.perforation || r.tabs) && <p className="hint">{t("cut.lineOrder")}</p>}
          <NumberField label={t("cut.kerf")} unit="mm" value={r.kerf_offset_mm} min={-50} max={50} digits={3} onCommit={num((kerf_offset_mm) => opt({ kerf_offset_mm }))} />
          {r.kerf_offset_mm !== 0 && <p className="hint">{t("cut.kerfHint")}</p>}
        </fieldset>
      )}

      <fieldset className="job-fieldset">
        <legend>{t("cut.pauses")}</legend>
        <NumberField label={t("cut.pauseBefore")} unit="ms" value={r.pause_before_ms} min={0} max={600000} digits={1} onCommit={num((pause_before_ms) => opt({ pause_before_ms }))} />
        <NumberField label={t("cut.pauseAfter")} unit="ms" value={r.pause_after_ms} min={0} max={600000} digits={1} onCommit={num((pause_after_ms) => opt({ pause_after_ms }))} />
      </fieldset>

      <fieldset className="job-fieldset">
        <legend>{t("cut.z")}</legend>
        <NumberField label={t("cut.zOffset")} unit="mm" value={r.z_offset_mm} min={-1000} max={1000} digits={3} onCommit={num((z_offset_mm) => opt({ z_offset_mm }))} />
        <NumberField label={t("cut.zStep")} unit="mm" value={r.z_step_mm} min={-100} max={100} digits={3} onCommit={num((z_step_mm) => opt({ z_step_mm }))} />
        {(r.z_offset_mm !== 0 || r.z_step_mm !== 0) && <p className="hint">{t("cut.zHint")}</p>}
      </fieldset>

      <details className="cut-timing" open={timing !== null || undefined}>
        <summary>{t("cut.timing")}</summary>
        <CheckField
          label={t("cut.overrideTimings")}
          checked={timing !== null}
          onChange={(on) => exec(timingPatch(ref, on ? (devTiming ?? deviceTiming(null)) : null))}
        />
        {timing ? (
          TIMING_KEYS.map((k) => (
            <NumberField
              key={k}
              label={TIMING_LABEL[k][0]}
              unit={TIMING_LABEL[k][1]}
              value={timing[k]}
              min={k === "jump_speed_mm_s" ? 1 : k === "laser_on_delay_us" ? undefined : 0}
              digits={k === "jump_distance_limit_mm" ? 3 : 1}
              onCommit={num((v) => exec(timingPatch(ref, { ...timing, [k]: v })))}
            />
          ))
        ) : (
          <p className="hint">{devTiming ? t("cut.timingDevice", { on: devTiming.laser_on_delay_us, off: devTiming.laser_off_delay_us, jump: devTiming.jump_speed_mm_s }) : t("cut.timingDeviceUnknown")}</p>
        )}
      </details>
    </details>
  );
}

// ---- shape properties -----------------------------------------------------------------------------------------------

export function ShapeCutProperties(p: { doc: GDocument; ids: string[]; run: Run; report: (e: unknown) => void; has: (cmd: string) => boolean }) {
  const t = useT();
  if (!p.has("object.set_cut_properties") || p.ids.length === 0) return null;
  const s = sharedOptions(p.doc, p.ids);
  const exec = (inv: Invocation) => void p.run(inv).catch(p.report);
  return (
    <section className="prop-section">
      <h3>{t("shape.title")}</h3>
      <NumberField
        label={t("shape.powerScale")}
        unit="%"
        value={s.power_scale_pct === "mixed" ? null : s.power_scale_pct}
        placeholder={t("shape.mixed")}
        min={0}
        max={100}
        digits={1}
        onCommit={(v) => v !== null && exec(setCutProperties(p.ids, { power_scale_pct: v }))}
      />
      <NumberField
        label={t("shape.priority")}
        value={s.priority === "mixed" || s.priority === null ? null : s.priority}
        placeholder={s.priority === "mixed" ? t("shape.mixed") : t("shape.noPriority")}
        min={0}
        max={1_000_000}
        digits={0}
        allowEmpty
        onCommit={(v) => exec(setCutProperties(p.ids, { priority: v === null ? null : Math.round(v) }))}
      />
      <p className="hint">{t("shape.hint")}</p>
    </section>
  );
}
