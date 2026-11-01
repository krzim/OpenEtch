// LightBurn devices: `useLightBurn` loads the engine's LightBurn device list (GET /api/lightburn) for the File dialogs
// and the Laser workspace, and `LightBurnDevices` is the small section of the Machine card that shows the device the
// engine matched, how the machine profile differs from it, and "Load LightBurn settings…" for engines on a computer
// without LightBurn (a Pi). The upload sends only the device list: api/lightburn.ts cuts it out of prefs.ini in the
// browser, so the rest of the file (other settings, secrets) never leaves this device.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError } from "../api/client.ts";
import {
  MAX_PREFS_FILE_BYTES,
  PREFS_LOCATIONS,
  devicePrefsBody,
  diffLabel,
  findDevice,
  formatDiffValue,
  lightburnInfo,
  machineLightBurn,
  type LightBurnInfo,
} from "../api/lightburn.ts";
import { formatBytes } from "../api/project.ts";
import type { MachineStatus } from "../api/types.ts";
import type { LinkState } from "../api/useEngine.ts";
import { fmt } from "./ui.tsx";
import { useT } from "../workspace/usePrefs.ts";

export interface LightBurnState {
  /** The engine's LightBurn devices, or null while loading or when the engine has no LightBurn routes. */
  info: LightBurnInfo | null;
  /** The engine answered 404: it is older than LightBurn device support. */
  unsupported: boolean;
  refresh: () => Promise<void>;
}

/** Load GET /api/lightburn on start, again whenever the event link reconnects, and on `refresh`. */
export function useLightBurn(link: LinkState): LightBurnState {
  const [info, setInfo] = useState<LightBurnInfo | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const refresh = useCallback(async () => {
    try {
      const res = await api.lightburn();
      if (!alive.current) return;
      setInfo(lightburnInfo(res));
      setUnsupported(false);
    } catch (e) {
      if (!alive.current) return;
      // An older engine has no route; any other failure keeps what was loaded (the connection indicator says why).
      if (e instanceof ApiError && (e.status === 404 || e.status === 405)) {
        setUnsupported(true);
        setInfo(null);
      }
    }
  }, []);
  useEffect(() => {
    if (link !== "closed") void refresh();
  }, [link, refresh]);
  return { info, unsupported, refresh };
}

/** The "LightBurn devices" section of the Machine card. */
export function LightBurnDevices(p: {
  lightburn: LightBurnState;
  machine: MachineStatus | null;
  /** After an upload or a forget: refetch the machine status (its `lightburn` diffs change). */
  onChanged: () => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
}) {
  const t = useT();
  const { info, unsupported } = p.lightburn;
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  if (unsupported) return null;
  const matched = machineLightBurn(p.machine);
  const devices = info?.devices ?? [];
  const matchedDevice = findDevice(devices, matched?.device ?? info?.default_device ?? null);
  const fieldOf = (mm: number | null) => (mm === null ? "" : `${fmt(mm, 2)} mm`);

  const upload = async (f: File) => {
    if (f.size > MAX_PREFS_FILE_BYTES) {
      p.notify(`${f.name} is larger than ${formatBytes(MAX_PREFS_FILE_BYTES)}; pick LightBurn's prefs.ini.`, "error");
      return;
    }
    setBusy(true);
    try {
      // Only the device list goes to the engine (see devicePrefsBody); the file's other settings stay here.
      const { body, devices: n } = devicePrefsBody(await f.text());
      await api.lightburnUpload(body);
      p.notify(n === 1 ? t("rest.lbDevice1") : t("rest.lbDevicesN", { n }));
      await p.lightburn.refresh();
      p.onChanged();
    } catch (e) {
      p.report(e);
    } finally {
      setBusy(false);
    }
  };

  const forget = async () => {
    setBusy(true);
    try {
      await api.lightburnForget();
      p.notify(t("lb.forgotTheUploadedLightburnSettings"));
      await p.lightburn.refresh();
      p.onChanged();
    } catch (e) {
      p.report(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="lb-devices">
      <h4>{t("lb.lightburnDevices")}</h4>
      <input
        ref={input}
        type="file"
        accept=".ini,.json,application/json,text/plain"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) void upload(f);
        }}
      />
      {info === null ? (
        <p className="muted">{t("lb.loading")}</p>
      ) : !info.available ? (
        <p className="muted">{t("lb.noLightburnSettingsOnTheEngine")}</p>
      ) : (
        <>
          <ul className="lb-device-list">
            {devices.map((d) => (
              <li key={d.name} className={d === matchedDevice ? "matched" : undefined}>
                <span className="lb-device-name">{d.name}</span>
                <span className="lb-device-meta">
                  {fieldOf(d.field_mm)}
                  {d.width_mm !== null && d.height_mm !== null && Math.abs(d.width_mm - d.height_mm) > 1e-3
                    ? ` (${fmt(d.width_mm, 2)} × ${fmt(d.height_mm, 2)})`
                    : ""}
                  {d.name === info.default_device ? " · default" : ""}
                  {d === matchedDevice ? " · used here" : ""}
                </span>
              </li>
            ))}
          </ul>
          <p className="hint">
            {info.source === "uploaded" ? t("lb.fromLightburnSettingsLoadedFromA") : t("lb.fromLightburnOnTheEngineComputer")}
          </p>
          {matched && matched.device && (
            <>
              {matched.diffs.length === 0 ? (
                <p className="hint">The machine profile matches {matched.device}.</p>
              ) : (
                <details className="lb-diffs" open={open} onToggle={(e) => setOpen((e.target as HTMLDetailsElement).open)}>
                  <summary>
                    {matched.diffs.length} difference{matched.diffs.length === 1 ? "" : "s"} from {matched.device}
                  </summary>
                  <table className="lb-diff-table">
                    <thead>
                      <tr>
                        <th scope="col">{t("lb.setting")}</th>
                        <th scope="col">{t("lb.openetch")}</th>
                        <th scope="col">{t("lb.lightburn")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {matched.diffs.map((d) => (
                        <tr key={d.field} title={d.note ?? undefined}>
                          <td>
                            {diffLabel(d.field)}
                            {d.note && <div className="lb-diff-note">{d.note}</div>}
                          </td>
                          <td>{formatDiffValue(d.base_value)}</td>
                          <td>{formatDiffValue(d.lightburn_value)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  <p className="hint">
                    The laser keeps using the machine profile. To use LightBurn's values, run “openetch profile
                    from-lightburn” on the engine computer.
                  </p>
                </details>
              )}
            </>
          )}
        </>
      )}
      <div className="lb-actions">
        <button className="btn" disabled={busy} onClick={() => input.current?.click()}>
          {t("lb.loadLightburnSettings")}
        </button>
        {info?.source === "uploaded" && (
          <button className="btn" disabled={busy} onClick={() => void forget()}>
            {t("lb.forget")}
          </button>
        )}
      </div>
      <p className="hint">
        Pick LightBurn's prefs.ini ({PREFS_LOCATIONS.map((l) => `${l.os}: ${l.path}`).join("; ")}). Only the device list
        is read and sent to the engine: device names, field sizes and galvo settings. Everything else in the file stays
        on this device.
      </p>
    </div>
  );
}
