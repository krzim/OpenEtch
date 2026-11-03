// Pre-flight warnings next to Start (short list #9): `job.preflight` on the document as it is now (and the active
// device's keep-out zones), shown above the Start button in the Laser dock and the phone's Job sheet, and as a pill
// beside the toolbar's Start. Each warning can be switched off for the project (`job.preflight_set`, saved with it);
// a keep-out finding is a refusal (the engine refuses the job), so it has no switch. One request per document
// revision is shared by every Start on the page.

import { useEffect, useSyncExternalStore } from "react";
import { execute } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import { Icon } from "../components/ui.tsx";
import { libsApi } from "../libs/api.ts";
import { OPEN_CONTACT_SHEET } from "../templates/useTemplateUi.tsx";
import { useLaserLive } from "../laser/useLaserLive.ts";
import { useT, useTD } from "../workspace/usePrefs.ts";
import { J, parsePreflight, preflightKey, splitFindings, type DeviceZone, type PreflightResult, type PreflightWarning } from "./jigModel.ts";
import "./jigs.css";
import { HelpLink } from "../guide/HelpLink.tsx";

// ---- shared stores: the device's keep-out zones and the findings per revision -------------------------------------

let zones: DeviceZone[] = [];
let zonesLoaded = false;
const zoneListeners = new Set<() => void>();

function setZones(z: DeviceZone[]) {
  zones = z;
  zonesLoaded = true;
  zoneListeners.forEach((f) => f());
}

/** Read the active device's keep-out zones from the engine's preferences (again on `prefs` and device events). */
export function loadDeviceZones(): void {
  void libsApi
    .prefs()
    .then((r) => setZones(((r as { device_prefs?: { keep_out?: DeviceZone[] } }).device_prefs?.keep_out ?? []) as DeviceZone[]))
    .catch(() => setZones([]));
}

let zoneUsers = 0;
let zoneUnlisten: (() => void) | null = null;

/** The active device's keep-out zones, kept current while mounted. */
export function useDeviceZones(): DeviceZone[] {
  const z = useSyncExternalStore(
    (f) => {
      zoneListeners.add(f);
      return () => zoneListeners.delete(f);
    },
    () => zones,
  );
  useEffect(() => {
    if (zoneUsers++ === 0) {
      if (!zonesLoaded) loadDeviceZones();
      zoneUnlisten = onEngineEvent((ev) => {
        const e = (ev as { event?: unknown }).event;
        if ((ev.type === "machine" && e === "prefs") || ev.type === "device_changed") loadDeviceZones();
      });
    }
    return () => {
      if (--zoneUsers === 0) {
        zoneUnlisten?.();
        zoneUnlisten = null;
      }
    };
  }, []);
  return z;
}

interface Found {
  key: string;
  result: PreflightResult | null;
}

let found: Found = { key: "", result: null };
let pendingKey = "";
const foundListeners = new Set<() => void>();
let timer: number | undefined;

function setFound(f: Found) {
  found = f;
  foundListeners.forEach((l) => l());
}

/** Check the document at `revision` (debounced; a newer request wins). */
function refresh(revision: number | null, z: readonly DeviceZone[]) {
  if (revision === null) return;
  const key = `${revision}|${JSON.stringify(z)}`;
  if (key === found.key || key === pendingKey) return;
  pendingKey = key;
  window.clearTimeout(timer);
  timer = window.setTimeout(() => {
    void execute(J.preflight(z))
      .then((r) => {
        if (pendingKey === key) setFound({ key, result: parsePreflight(r.result) });
      })
      .catch(() => {
        // An engine without job.preflight (older) or a refused zone list: no warnings rather than an error box.
        if (pendingKey === key) setFound({ key, result: null });
      });
  }, 250);
}

/** The pre-flight findings of the document the page shows. */
export function usePreflight(): PreflightResult | null {
  const { facts } = useLaserLive();
  const z = useDeviceZones();
  const f = useSyncExternalStore(
    (l) => {
      foundListeners.add(l);
      return () => foundListeners.delete(l);
    },
    () => found,
  );
  useEffect(() => refresh(facts.revision, z), [facts.revision, z]);
  return f.result;
}

// ---- the warnings next to Start --------------------------------------------------------------------------------------

function useFindingText() {
  const td = useTD();
  return (w: PreflightWarning) => {
    const k = preflightKey(w);
    return k.key ? td(k.key, w.text, k.vars) : w.text;
  };
}

/** Switch a check off for the project (or every switched-off one back on). */
function mute(check: string) {
  void execute(J.preflightSet(check, false)).catch(() => undefined);
}

export function PreflightNotes({ variant }: { variant: "toolbar" | "dock" | "sheet" }) {
  const t = useT();
  const td = useTD();
  const text = useFindingText();
  const { facts } = useLaserLive();
  const r = usePreflight();
  if (!r) return null;
  const { refusals, warnings } = splitFindings(r);
  const n = refusals.length + warnings.length;
  if (variant === "toolbar") {
    if (n === 0) return null;
    const all = [...refusals, ...warnings].map(text).join("\n");
    const label = refusals.length ? t("pf.refused") : warnings.length === 1 ? t("pf.one") : t("pf.count", { n });
    return (
      <button
        type="button"
        className={`pf-pill${refusals.length ? " pf-pill-refused" : ""}`}
        title={all}
        aria-label={`${label}: ${all}`}
        // The full list with its switches is in the Laser window (the phone: the Job sheet).
        onClick={() => facts.openLaser()}
      >
        <Icon name="warning" size={14} />
        <span>{refusals.length ? t("pf.refused") : n}</span>
      </button>
    );
  }
  const disabled = r.disabled.filter((c) => c !== "keep_out");
  if (n === 0 && disabled.length === 0) return null;
  return (
    <div className={`pf-notes pf-notes-${variant}`} role="status" aria-live="polite">
      {n > 0 && <div className="pf-head">{refusals.length ? t("pf.refused") : warnings.length === 1 ? t("pf.one") : t("pf.count", { n })} <HelpLink to="preflight-and-keep-out#pre-flight-warnings" topic={t("guide.topic.preflight")} /></div>}
      <ul className="pf-list">
        {refusals.map((w, i) => (
          <li key={`r${i}`} className="pf-item pf-item-refused">
            <Icon name="warning" size={14} />
            <span className="pf-text">{text(w)}</span>
          </li>
        ))}
        {warnings.map((w, i) => (
          <li key={`w${i}`} className="pf-item">
            <Icon name="warning" size={14} />
            <span className="pf-text">{text(w)}</span>
            <span className="pf-actions">
              {/* The contact sheet's findings open it, rather than naming the menu path. */}
              {w.check === "contact_sheet" && (
                <button type="button" className="link-btn pf-mute" onClick={() => window.dispatchEvent(new CustomEvent(OPEN_CONTACT_SHEET))} title={t("sheet.openTitle")}>
                  {t("sheet.menu")}
                </button>
              )}
              <button type="button" className="link-btn pf-mute" onClick={() => mute(w.check)} title={td(`pf.check.${w.check}`, w.check)}>
                {t("pf.mute")}
              </button>
            </span>
          </li>
        ))}
      </ul>
      {disabled.length > 0 && (
        <button
          type="button"
          className="link-btn pf-restore"
          title={disabled.map((c) => td(`pf.check.${c}`, c)).join(", ")}
          onClick={() => void Promise.all(disabled.map((c) => execute(J.preflightSet(c, true)))).catch(() => undefined)}
        >
          {t("pf.restore", { n: disabled.length })}
        </button>
      )}
    </div>
  );
}
