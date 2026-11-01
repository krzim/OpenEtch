// Devices, LightBurn-style: `useDevices` loads the engine's device registry (GET /api/devices) on start, on reconnect
// and on `device_changed`; `DevicesWindow` is LightBurn's Devices window (the list with the active device starred,
// Import from LightBurn on this computer, Import file for .lbdev and prefs.ini, Remove, Make active, and a read-only
// view of the selected device's settings and how OpenEtch's profile differs from its base); `DeviceBar` is the device
// dropdown at the top of the Laser workspace's Machine card, like the one in LightBurn's Laser window.
//
// Switching devices changes the machine (the engine swaps the profile and re-initialises the controller), so it always
// asks first and is disabled while a run or a frame is under way. Imported files are cut down to the typed device
// fields in this browser (api/devices.ts) before they are sent.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, ApiError } from "../api/client.ts";
import {
  MAX_DEVICE_FILE_BYTES,
  activateBlocker,
  activeDevice,
  designOnlyReason,
  deviceDetails,
  deviceImportBody,
  deviceList,
  deviceOptionText,
  importAddParam,
  importMessage,
  importResult,
  isGalvo,
  keepSelection,
  kindLabel,
  originLabel,
  removeBlocker,
  sourceLabel,
  switchBlocker,
  switchMessage,
  workspaceText,
  type DeviceDetails,
  type DeviceImportBody,
  type DeviceList,
  type DeviceSummary,
} from "../api/devices.ts";
import { diffLabel, formatDiffValue } from "../api/lightburn.ts";
import { formatBytes } from "../api/project.ts";
import type { MachineStatus } from "../api/types.ts";
import type { LinkState } from "../api/useEngine.ts";
import { Icon, Modal } from "./ui.tsx";

type Notify = (msg: string, kind?: "info" | "error") => void;

export interface DevicesState {
  /** The registry, or null while loading or when the engine has no device routes. */
  list: DeviceList | null;
  /** The engine answered 404: it is older than device support (the UI falls back to GET /api/lightburn). */
  unsupported: boolean;
  refresh: () => Promise<void>;
}

/** Load GET /api/devices on start, when the event link reconnects, on every `device_changed` (`seq`) and on `refresh`. */
export function useDevices(link: LinkState, seq: number): DevicesState {
  const [list, setList] = useState<DeviceList | null>(null);
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
      const res = await api.devices();
      if (!alive.current) return;
      setList(deviceList(res));
      setUnsupported(false);
    } catch (e) {
      if (!alive.current) return;
      // An older engine has no route; any other failure keeps what was loaded (the connection indicator says why).
      if (e instanceof ApiError && (e.status === 404 || e.status === 405)) {
        setUnsupported(true);
        setList(null);
      }
    }
  }, []);
  useEffect(() => {
    if (link !== "closed") void refresh();
  }, [link, seq, refresh]);
  return { list, unsupported, refresh };
}

/** Make `device` active, then refetch what depends on it. Resolves true when the engine switched. */
async function activate(device: DeviceSummary, p: { devices: DevicesState; onSwitched: () => void; notify: Notify; report: (e: unknown) => void }): Promise<boolean> {
  try {
    await api.deviceActivate(device.id);
    p.notify(`Active device: ${device.display_name}.`);
    return true;
  } catch (e) {
    p.report(e);
    return false;
  } finally {
    await p.devices.refresh();
    p.onSwitched();
  }
}

/** The confirmation shown before switching devices (inline in the Devices window, as a dialog from the dropdown). */
function SwitchConfirm(p: { next: DeviceSummary; current: DeviceSummary | null; blocker: string | null; busy: boolean; onCancel: () => void; onConfirm: () => void }) {
  return (
    <div className="dev-confirm" role="alertdialog" aria-label={`Switch to ${p.next.display_name}`}>
      <p>{switchMessage(p.next, p.current)}</p>
      {p.blocker && <p className="form-error">{p.blocker}</p>}
      <div className="dev-confirm-actions">
        <button className="btn" onClick={p.onCancel}>
          Cancel
        </button>
        <button className="btn btn-primary" disabled={p.busy || p.blocker !== null} onClick={p.onConfirm}>
          Switch device
        </button>
      </div>
    </div>
  );
}

/** The Laser workspace's device dropdown (LightBurn's Laser window), with "Devices…" and the design-only notice. */
export function DeviceBar(p: {
  devices: DevicesState;
  machine: MachineStatus | null;
  onOpenWindow: () => void;
  /** After a switch: refetch the machine status and the document. */
  onSwitched: () => void;
  notify: Notify;
  report: (e: unknown) => void;
}) {
  const { list } = p.devices;
  const [pending, setPending] = useState<DeviceSummary | null>(null);
  const [busy, setBusy] = useState(false);
  const active = activeDevice(list);
  const blocker = switchBlocker(p.machine);
  const designOnly = designOnlyReason(active);
  return (
    <div className="dev-bar">
      <div className="dev-bar-row">
        <label className="dev-select dev-select-bar">
          <select
            aria-label="Active device"
            value={active?.id ?? ""}
            disabled={!list || list.devices.length === 0 || blocker !== null || busy}
            title={blocker ?? "The device you are working with"}
            onChange={(e) => {
              const next = list?.devices.find((d) => d.id === e.target.value) ?? null;
              if (next && next.id !== active?.id) setPending(next);
            }}
          >
            {!active && <option value="">{list ? "No active device" : "Loading…"}</option>}
            {list?.devices.map((d) => (
              <option key={d.id} value={d.id}>
                {deviceOptionText(d)}
              </option>
            ))}
          </select>
        </label>
        <button className="btn" onClick={p.onOpenWindow} title="Add, remove and inspect devices">
          Devices…
        </button>
      </div>
      {blocker && <p className="hint">Switching devices waits: {blocker}</p>}
      {designOnly && (
        <p className="run-hint" role="status">
          {designOnly}
        </p>
      )}
      {pending && (
        <Modal title="Switch device" onClose={() => setPending(null)}>
          <SwitchConfirm
            next={pending}
            current={active}
            blocker={switchBlocker(p.machine)}
            busy={busy}
            onCancel={() => setPending(null)}
            onConfirm={() => {
              const next = pending;
              setBusy(true);
              void activate(next, { devices: p.devices, onSwitched: p.onSwitched, notify: p.notify, report: p.report }).finally(() => {
                setBusy(false);
                setPending(null);
              });
            }}
          />
        </Modal>
      )}
    </div>
  );
}

/** What the Devices window footer is asking: nothing, to switch, to remove, or which devices of a file to import. */
type Ask =
  | { kind: "switch"; device: DeviceSummary }
  | { kind: "remove"; device: DeviceSummary }
  | { kind: "import"; file: string; parsed: DeviceImportBody; choice: string };

/** LightBurn's Devices window. */
export function DevicesWindow(p: {
  devices: DevicesState;
  machine: MachineStatus | null;
  onClose: () => void;
  onSwitched: () => void;
  notify: Notify;
  report: (e: unknown) => void;
}) {
  const { list } = p.devices;
  const [selected, setSelected] = useState<string | null>(() => (list ? keepSelection(list, null) : null));
  const [details, setDetails] = useState<{ id: string; d: DeviceDetails | null; error: string | null } | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  // Keep a selection that still exists after the list changes (an import, a removal, a switch elsewhere).
  useEffect(() => {
    if (list) setSelected((s) => keepSelection(list, s));
  }, [list]);

  const device = list?.devices.find((d) => d.id === selected) ?? null;
  const active = activeDevice(list);

  // Load the selected device's settings.
  useEffect(() => {
    if (!selected) return;
    let alive = true;
    api
      .device(selected)
      .then((res) => alive && setDetails({ id: selected, d: deviceDetails(res), error: null }))
      .catch((e: unknown) => alive && setDetails({ id: selected, d: null, error: e instanceof ApiError ? (e.detail ?? e.message) : String(e) }));
    return () => {
      alive = false;
    };
  }, [selected, list]);

  const task = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      p.report(e);
    } finally {
      setBusy(false);
    }
  };

  const runImport = (body: string | null, add: string, what: string) =>
    task(async () => {
      const r = importResult(await api.deviceImport(body, add));
      const m = importMessage(r);
      p.notify(`${what}: ${m.msg}`, m.kind);
      setAsk(null);
      await p.devices.refresh();
      // Select the first device the import added or updated, by its name.
      const first = r.added.find(Boolean) ?? r.updated.find(Boolean);
      if (first) {
        const res = deviceList(await api.devices().catch(() => null));
        const hit = res.devices.find((d) => d.display_name === first || d.name === first || d.id === first);
        if (hit) setSelected(hit.id);
      }
    });

  const pickFile = async (f: File) => {
    if (f.size > MAX_DEVICE_FILE_BYTES) {
      p.notify(`${f.name} is larger than ${formatBytes(MAX_DEVICE_FILE_BYTES)}; pick a .lbdev file or LightBurn's prefs.ini.`, "error");
      return;
    }
    try {
      // Only the typed device fields leave this browser (see deviceImportBody).
      const parsed = deviceImportBody(await f.text());
      if (parsed.names.length === 1) void runImport(parsed.body, "all", f.name);
      else setAsk({ kind: "import", file: f.name, parsed, choice: "" });
    } catch (e) {
      p.report(e);
    }
  };

  const removeIt = (d: DeviceSummary) =>
    task(async () => {
      await api.deviceRemove(d.id);
      p.notify(`Removed ${d.display_name}.`);
      setAsk(null);
      await p.devices.refresh();
    });

  const activateBlock = activateBlocker(device, list, p.machine);
  const removeBlock = removeBlocker(device, list);

  return (
    <Modal
      title="Devices"
      wide
      onClose={p.onClose}
      footer={
        ask?.kind === "switch" ? (
          <SwitchConfirm
            next={ask.device}
            current={active}
            blocker={activateBlocker(ask.device, list, p.machine)}
            busy={busy}
            onCancel={() => setAsk(null)}
            onConfirm={() => {
              setBusy(true);
              void activate(ask.device, { devices: p.devices, onSwitched: p.onSwitched, notify: p.notify, report: p.report }).finally(() => {
                setBusy(false);
                setAsk(null);
              });
            }}
          />
        ) : ask?.kind === "remove" ? (
          <div className="dev-confirm" role="alertdialog" aria-label={`Remove ${ask.device.display_name}`}>
            <p>
              Remove {ask.device.display_name}? Make sure you have the right device selected. Its LightBurn settings stay
              in LightBurn; you can import it again.
            </p>
            <div className="dev-confirm-actions">
              <button className="btn" onClick={() => setAsk(null)}>
                Cancel
              </button>
              <button className="btn btn-warn" disabled={busy} onClick={() => void removeIt(ask.device)}>
                Remove
              </button>
            </div>
          </div>
        ) : ask?.kind === "import" ? (
          <div className="dev-confirm">
            <label className="dev-select">
              <span className="dev-select-label">Import from {ask.file}</span>
              <select value={ask.choice} onChange={(e) => setAsk({ ...ask, choice: e.target.value })}>
                <option value="">All {ask.parsed.names.length} devices</option>
                {ask.parsed.names.map((n, i) => (
                  <option key={`${n}-${i}`} value={n}>
                    {n}
                    {i === ask.parsed.defaultIndex ? " (LightBurn default)" : ""}
                  </option>
                ))}
              </select>
            </label>
            <div className="dev-confirm-actions">
              <button className="btn" onClick={() => setAsk(null)}>
                Cancel
              </button>
              <button className="btn btn-primary" disabled={busy} onClick={() => void runImport(ask.parsed.body, importAddParam(ask.choice), ask.file)}>
                Import
              </button>
            </div>
          </div>
        ) : (
          <>
            <button className="btn" disabled={busy} onClick={() => void runImport(null, "all", "LightBurn on the engine computer")}>
              Import from LightBurn on this computer
            </button>
            <button className="btn" disabled={busy} onClick={() => input.current?.click()}>
              <Icon name="upload" /> Import file…
            </button>
            <button className="btn" disabled={busy || removeBlock !== null} title={removeBlock ?? "Remove the selected device"} onClick={() => device && setAsk({ kind: "remove", device })}>
              Remove
            </button>
            <button
              className="btn btn-primary"
              disabled={busy || activateBlock !== null}
              title={activateBlock ?? "Work with the selected device"}
              onClick={() => device && setAsk({ kind: "switch", device })}
            >
              Make active
            </button>
            <button className="btn" onClick={p.onClose}>
              Close
            </button>
          </>
        )
      }
    >
      <input
        ref={input}
        type="file"
        accept=".lbdev,.ini,.json,application/json,text/plain"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) void pickFile(f);
        }}
      />
      {list === null ? (
        <p className="muted">Loading…</p>
      ) : list.devices.length === 0 ? (
        <p className="muted">No devices yet. Import them from LightBurn.</p>
      ) : (
        <div className="dev-table-wrap">
          <table className="dev-table" role="grid" aria-label="Devices">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Controller</th>
                <th scope="col">Size</th>
                <th scope="col">Origin</th>
                <th scope="col">Source</th>
              </tr>
            </thead>
            <tbody>
              {list.devices.map((d) => (
                <tr
                  key={d.id}
                  className={`${d.id === selected ? "selected" : ""}${d.id === list.active ? " active" : ""}`}
                  aria-selected={d.id === selected}
                  tabIndex={0}
                  onClick={() => setSelected(d.id)}
                  onDoubleClick={() => {
                    setSelected(d.id);
                    if (activateBlocker(d, list, p.machine) === null) setAsk({ kind: "switch", device: d });
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setSelected(d.id);
                    }
                  }}
                >
                  <td>
                    <span className="dev-name">
                      {d.id === list.active && (
                        <span className="dev-star" title="Active device" aria-label="Active device">
                          *
                        </span>
                      )}
                      {d.display_name}
                    </span>
                    {!d.controllable && <span className="dev-tag">design only</span>}
                  </td>
                  <td>
                    {d.name}
                    {d.lightburn_type ? <span className="dev-sub"> · {d.lightburn_type}</span> : null}
                  </td>
                  <td>{workspaceText(isGalvo(d.kind) ? [d.workspace_mm[0], d.workspace_mm[0]] : d.workspace_mm)}</td>
                  <td>{originLabel(d.origin)}</td>
                  <td>{sourceLabel(d.source)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {device && <DeviceSettings device={device} details={details && details.id === device.id ? details : null} />}
      <p className="hint">
        Import from LightBurn reads LightBurn's prefs.ini on the engine computer; Import file takes a .lbdev file or a
        prefs.ini from another computer. Only device settings are read: names, sizes, origin and galvo settings. Nothing
        is written back to LightBurn.
      </p>
    </Modal>
  );
}

/** The read-only summary of the selected device (LightBurn 2.x shows one under the list). */
function DeviceSettings(p: { device: DeviceSummary; details: { d: DeviceDetails | null; error: string | null } | null }) {
  const { device } = p;
  const d = p.details?.d ?? null;
  const groups = new Map<string, { label: string; value: unknown }[]>();
  for (const s of d?.settings ?? []) {
    const g = groups.get(s.group) ?? [];
    g.push(s);
    groups.set(s.group, g);
  }
  const designOnly = designOnlyReason(device);
  return (
    <section className="dev-settings" aria-label={`${device.display_name} settings`}>
      <h4>{device.display_name}</h4>
      <dl className="kv">
        <dt>Controller</dt>
        <dd>{device.name}</dd>
        <dt>Connection</dt>
        <dd>{device.lightburn_type ?? "–"}</dd>
        <dt>Kind</dt>
        <dd>{kindLabel(device.kind)}</dd>
        <dt>{isGalvo(device.kind) ? "Field" : "Workspace"}</dt>
        <dd>{workspaceText(isGalvo(device.kind) ? [device.workspace_mm[0], device.workspace_mm[0]] : device.workspace_mm)}</dd>
        <dt>Origin</dt>
        <dd>{originLabel(device.origin)}</dd>
        <dt>Source</dt>
        <dd>{sourceLabel(device.source)}</dd>
        <dt>Runs jobs</dt>
        <dd>{device.controllable ? "Yes" : "No (design only)"}</dd>
        {d?.base && (
          <>
            <dt>Profile base</dt>
            <dd>{d.base}</dd>
          </>
        )}
      </dl>
      {designOnly && <p className="run-hint">{designOnly}</p>}
      {p.details === null ? (
        <p className="muted">Loading settings…</p>
      ) : p.details.error ? (
        <p className="form-error">{p.details.error}</p>
      ) : (
        d && (
          <>
            {d.profile.length > 0 && (
              <details className="dev-group">
                <summary>OpenEtch profile</summary>
                <dl className="kv">
                  {d.profile.map((r) => (
                    <SettingRow key={r.label} label={r.label} value={r.value} />
                  ))}
                </dl>
              </details>
            )}
            {[...groups.entries()].map(([g, rows]) => (
              <details key={g} className="dev-group">
                <summary>{g}</summary>
                <dl className="kv">
                  {rows.map((r) => (
                    <SettingRow key={r.label} label={r.label} value={r.value} />
                  ))}
                </dl>
              </details>
            ))}
            {d.diffs.length > 0 ? (
              <details className="lb-diffs">
                <summary>
                  {d.diffs.length} difference{d.diffs.length === 1 ? "" : "s"} from {d.base ?? "the base profile"}
                </summary>
                <table className="lb-diff-table">
                  <thead>
                    <tr>
                      <th scope="col">Setting</th>
                      <th scope="col">Base</th>
                      <th scope="col">This device</th>
                    </tr>
                  </thead>
                  <tbody>
                    {d.diffs.map((x) => (
                      <tr key={x.field} title={x.note ?? undefined}>
                        <td>
                          {diffLabel(x.field)}
                          {x.note && <div className="lb-diff-note">{x.note}</div>}
                        </td>
                        <td>{formatDiffValue(x.base_value)}</td>
                        <td>{formatDiffValue(x.lightburn_value)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </details>
            ) : (
              device.controllable && <p className="hint">The profile matches {d.base ?? "its base"}.</p>
            )}
          </>
        )
      )}
    </section>
  );
}

function SettingRow({ label, value }: { label: string; value: unknown }) {
  return (
    <>
      <dt>{diffLabel(label)}</dt>
      <dd>{formatDiffValue(value)}</dd>
    </>
  );
}
