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
import { api, ApiError, type Invocation } from "../api/client.ts";
import {
  MAX_DEVICE_FILE_BYTES,
  activateBlocker,
  activeDevice,
  designOnlyReason,
  deviceDetails,
  deviceGroups,
  deviceImportBody,
  deviceList,
  deviceOptionText,
  deviceSizeText,
  GRBL_FORM_DEFAULT,
  grblDeviceBody,
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
  type DeviceDetails,
  type DeviceImportBody,
  type DeviceList,
  type DeviceSummary,
  type GrblForm,
} from "../api/devices.ts";
import { diffLabel, formatDiffValue } from "../api/lightburn.ts";
import { RUIDA_FORM_DEFAULT, ruidaDeviceBody, type RuidaForm } from "../api/ruida.ts";
import { formatBytes } from "../api/project.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import { jobApi } from "../api/jobApi.ts";
import type { LinkState } from "../api/useEngine.ts";
import { DeviceEditor } from "./DeviceEditor.tsx";
import { Modal } from "./ui.tsx";
import { MenuButton, type MenuItem } from "./Menu.tsx";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { useT } from "../workspace/usePrefs.ts";
import { tr } from "../workspace/tr.ts";
import { errorText } from "../api/errorText.ts";

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
    p.notify(tr("dv.activeDeviceV1", { v1: device.display_name }));
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
  const t = useT();
  return (
    <div className="dev-confirm" role="alertdialog" aria-label={t("dv.switchToV1", { v1: p.next.display_name })}>
      <p>{switchMessage(p.next, p.current)}</p>
      {p.blocker && <p className="form-error">{p.blocker}</p>}
      <div className="dev-confirm-actions">
        <button className="btn" onClick={p.onCancel}>
          {t("dv.cancel")}
        </button>
        <button className="btn btn-primary" disabled={p.busy || p.blocker !== null} onClick={p.onConfirm}>
          {t("dv.switchDevice")}
        </button>
      </div>
    </div>
  );
}

/** The Laser workspace's device dropdown (LightBurn's Laser window), with "Devices…" and the design-only notice. */
export function DeviceBar(p: {
  devices: DevicesState;
  machine: MachineStatus | null;
  /** Opens the Devices window; without it (the phone, where device editing is a desktop feature) no "Devices…". */
  onOpenWindow?: () => void;
  /** After a switch: refetch the machine status and the document. */
  onSwitched: () => void;
  notify: Notify;
  report: (e: unknown) => void;
}) {
  const t = useT();
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
            aria-label={t("dv.activeDevice")}
            value={active?.id ?? ""}
            disabled={!list || list.devices.length === 0 || blocker !== null || busy}
            title={blocker ?? t("dv.theDeviceYouAreWorkingWith")}
            onChange={(e) => {
              const next = list?.devices.find((d) => d.id === e.target.value) ?? null;
              if (next && next.id !== active?.id) setPending(next);
            }}
          >
            {!active && <option value="">{list ? t("dv.noActiveDevice") : "Loading…"}</option>}
            {list?.devices.map((d) => (
              <option key={d.id} value={d.id}>
                {deviceOptionText(d)}
              </option>
            ))}
          </select>
        </label>
        {p.onOpenWindow && (
          <button className="btn" onClick={p.onOpenWindow} title={t("dv.addRemoveAndInspectDevices")}>
            {t("dv.devices")}
          </button>
        )}
      </div>
      {blocker && <p className="hint">{t("dv.switchWaits", { why: blocker })}</p>}
      {designOnly && (
        <p className="run-hint" role="status">
          {designOnly}
        </p>
      )}
      {pending && (
        <Modal title={t("dv.switchDevice")} onClose={() => setPending(null)}>
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

/** The New GRBL device form (Devices window footer): connection, bed, origin, homing, $30, feed limit, M4/M3. */
function GrblDeviceForm(p: { form: GrblForm; busy: boolean; onChange: (f: GrblForm) => void; onCancel: () => void; onAdd: () => void }) {
  const t = useT();
  const f = p.form;
  const text = (key: keyof GrblForm, label: string, mode: "text" | "decimal" = "decimal") => (
    <label className="dev-select">
      <span className="dev-select-label">{label}</span>
      <input inputMode={mode} value={String(f[key])} onChange={(e) => p.onChange({ ...f, [key]: e.target.value })} />
    </label>
  );
  return (
    <div className="dev-confirm" role="group" aria-label={t("dv.newGrblHint")}>
      {text("displayName", t("dv.grblName"), "text")}
      {text("port", t("dv.grblPort"), "text")}
      {text("baud", t("dv.grblBaud"))}
      {text("widthMm", t("dv.grblWidth"))}
      {text("heightMm", t("dv.grblHeight"))}
      <label className="dev-select">
        <span className="dev-select-label">{t("dv.grblOrigin")}</span>
        <select value={f.origin} onChange={(e) => p.onChange({ ...f, origin: e.target.value as GrblForm["origin"] })}>
          {(["bottom_left", "top_left", "bottom_right", "top_right"] as const).map((o) => (
            <option key={o} value={o}>
              {originLabel(o)}
            </option>
          ))}
        </select>
      </label>
      {text("sMax", t("dv.grblSMax"))}
      {text("maxFeedMmMin", t("dv.grblMaxFeed"))}
      <label>
        <input type="checkbox" checked={f.homing} onChange={(e) => p.onChange({ ...f, homing: e.target.checked })} /> {t("dv.grblHoming")}
      </label>
      <label>
        <input type="checkbox" checked={f.dynamicPower} onChange={(e) => p.onChange({ ...f, dynamicPower: e.target.checked })} /> {t("dv.grblDynamic")}
      </label>
      <div className="dev-confirm-actions">
        <button className="btn" onClick={p.onCancel}>
          {t("dv.cancel")}
        </button>
        <button className="btn btn-primary" disabled={p.busy} onClick={p.onAdd}>
          {t("dv.add")}
        </button>
      </div>
    </div>
  );
}

/** The New Ruida device form (Devices window footer): controller address, bed, origin, power and speed limits. */
function RuidaDeviceForm(p: { form: RuidaForm; busy: boolean; onChange: (f: RuidaForm) => void; onCancel: () => void; onAdd: () => void }) {
  const t = useT();
  const f = p.form;
  const text = (key: "displayName" | "address" | "widthMm" | "heightMm" | "maxPowerPct" | "maxSpeedMmS", label: string, mode: "text" | "decimal" = "decimal") => (
    <label className="dev-select">
      <span className="dev-select-label">{label}</span>
      <input inputMode={mode} value={f[key]} onChange={(e) => p.onChange({ ...f, [key]: e.target.value })} />
    </label>
  );
  return (
    <div className="dev-confirm" role="group" aria-label={t("dv.newRuidaHint")}>
      {text("displayName", t("dv.grblName"), "text")}
      {text("address", t("dv.ruidaAddress"), "text")}
      {text("widthMm", t("dv.grblWidth"))}
      {text("heightMm", t("dv.grblHeight"))}
      <label className="dev-select">
        <span className="dev-select-label">{t("dv.grblOrigin")}</span>
        <select value={f.origin} onChange={(e) => p.onChange({ ...f, origin: e.target.value as RuidaForm["origin"] })}>
          {(["top_right", "top_left", "bottom_left", "bottom_right"] as const).map((o) => (
            <option key={o} value={o}>
              {originLabel(o)}
            </option>
          ))}
        </select>
      </label>
      {text("maxPowerPct", t("dv.ruidaMaxPower"))}
      {text("maxSpeedMmS", t("dv.ruidaMaxSpeed"))}
      <label>
        <input type="checkbox" checked={f.rdc634xg} onChange={(e) => p.onChange({ ...f, rdc634xg: e.target.checked })} /> {t("dv.ruida634xg")}
      </label>
      <div className="dev-confirm-actions">
        <button className="btn" onClick={p.onCancel}>
          {t("dv.cancel")}
        </button>
        <button className="btn btn-primary" disabled={p.busy} onClick={p.onAdd}>
          {t("dv.add")}
        </button>
      </div>
    </div>
  );
}

/** What the Devices window footer is asking: nothing, to switch, to remove, or which devices of a file to import. */
type Ask =
  | { kind: "switch"; device: DeviceSummary }
  | { kind: "remove"; device: DeviceSummary }
  | { kind: "import"; file: string; parsed: DeviceImportBody; choice: string }
  | { kind: "grbl"; form: GrblForm }
  | { kind: "ruida"; form: RuidaForm };

/** LightBurn's Devices window. */
export function DevicesWindow(p: {
  devices: DevicesState;
  machine: MachineStatus | null;
  onClose: () => void;
  onSwitched: () => void;
  notify: Notify;
  report: (e: unknown) => void;
  /** For the device settings editor's lens calibration wizard (the test pattern's layer). */
  doc?: GDocument | null;
  run?: (inv: Invocation | Invocation[]) => Promise<unknown>;
}) {
  const t = useT();
  const { list } = p.devices;
  const [selected, setSelected] = useState<string | null>(() => (list ? keepSelection(list, null) : null));
  const [details, setDetails] = useState<{ id: string; d: DeviceDetails | null; error: string | null } | null>(null);
  const [ask, setAsk] = useState<Ask | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const markcfgInput = useRef<HTMLInputElement>(null);

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
      .catch((e: unknown) => alive && setDetails({ id: selected, d: null, error: errorText(e) }));
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
      p.notify(t("dv.v1IsLargerThanV2Pick", { v1: f.name, v2: formatBytes(MAX_DEVICE_FILE_BYTES) }), "error");
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
      p.notify(t("rest.devRemoved", { name: d.display_name }));
      setAsk(null);
      await p.devices.refresh();
    });

  const duplicate = (d: DeviceSummary) =>
    task(async () => {
      const r = (await jobApi.deviceDuplicate(d.id)) as { device?: { id?: string } };
      p.notify(t("rest.devDuplicated", { name: d.display_name }));
      await p.devices.refresh();
      if (r?.device?.id) setSelected(r.device.id);
    });

  const importMarkcfg = (f: File) =>
    task(async () => {
      if (f.size > MAX_DEVICE_FILE_BYTES) throw new Error(t("dv.v1IsTooLargeForAn", { v1: f.name }));
      const r = (await jobApi.importMarkcfg(f, f.name.replace(/\.[^.]*$/, ""))) as { device?: { id?: string; display_name?: string } };
      p.notify(t("rest.devAdded", { name: r?.device?.display_name ?? t("dv.aDevice"), file: f.name }));
      await p.devices.refresh();
      if (r?.device?.id) setSelected(r.device.id);
    });

  const addGrbl = (form: GrblForm) =>
    task(async () => {
      const b = grblDeviceBody(form);
      if ("error" in b) throw new Error(b.error);
      const r = (await jobApi.addGrblDevice(b.body)) as { device?: { id?: string; display_name?: string } };
      p.notify(t("dv.grblAdded", { name: r?.device?.display_name ?? t("dv.aDevice") }));
      setAsk(null);
      await p.devices.refresh();
      if (r?.device?.id) setSelected(r.device.id);
    });

  const addRuida = (form: RuidaForm) =>
    task(async () => {
      const b = ruidaDeviceBody(form);
      if ("error" in b) throw new Error(b.error);
      const r = (await jobApi.addRuidaDevice(b.body)) as { device?: { id?: string; display_name?: string } };
      p.notify(t("dv.ruidaAdded", { name: r?.device?.display_name ?? t("dv.aDevice") }));
      setAsk(null);
      await p.devices.refresh();
      if (r?.device?.id) setSelected(r.device.id);
    });

  const activateBlock = activateBlocker(device, list, p.machine);
  const removeBlock = removeBlocker(device, list);
  const phone = useLayoutMode().layout === "phone";
  const addItems: MenuItem[] = [
    { label: t("dv.importFromLightburnOnThisComputer"), action: () => void runImport(null, "all", t("dv.lightburnOnTheEngineComputer")) },
    { label: t("dv.importFile"), icon: "upload", action: () => input.current?.click() },
    { label: t("dv.importEzcad"), title: t("dv.addADeviceFromAnEzcad"), action: () => markcfgInput.current?.click() },
    { sep: true },
    { label: t("dv.newGrbl"), title: t("dv.newGrblHint"), action: () => setAsk({ kind: "grbl", form: GRBL_FORM_DEFAULT }) },
    { label: t("dv.newRuida"), title: t("dv.newRuidaHint"), action: () => setAsk({ kind: "ruida", form: RUIDA_FORM_DEFAULT }) },
  ];
  const deviceItems: MenuItem[] = [
    { label: t("dv.duplicate"), title: t("dv.editACopyOfTheSelected"), disabled: !device, action: () => device && void duplicate(device) },
    { label: t("dv.edit"), title: t("dv.deviceSettingsFieldRedDotGalvo"), disabled: !device, action: () => setEditing(true) },
    {
      label: t("dv.remove"),
      title: removeBlock ?? t("dv.removeTheSelectedDevice"),
      disabled: removeBlock !== null,
      danger: true,
      action: () => device && setAsk({ kind: "remove", device }),
    },
  ];
  const groups = list ? deviceGroups(list) : [];

  return (
    <Modal
      title={t("dv.devices")}
      wide
      // While the device settings are open, Escape closes only them (every Modal listens on the window).
      onClose={editing ? noopClose : p.onClose}
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
              {t("dv.removeAsk", { name: ask.device.display_name })}{" "}
              {/* Only an imported device can be imported again (a GRBL or Ruida added by hand said "Its LightBurn settings stay in LightBurn"). */}
              {ask.device.source === "lightburn-prefs" ? t("dv.removeLightBurn") : ask.device.source === "lbdev" ? t("dv.removeLbdev") : t("dv.removeManual")}
            </p>
            <div className="dev-confirm-actions">
              <button className="btn" onClick={() => setAsk(null)}>
                {t("dv.cancel")}
              </button>
              <button className="btn btn-warn" disabled={busy} onClick={() => void removeIt(ask.device)}>
                {t("dv.remove")}
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
                {t("dv.cancel")}
              </button>
              <button className="btn btn-primary" disabled={busy} onClick={() => void runImport(ask.parsed.body, importAddParam(ask.choice), ask.file)}>
                {t("dv.import")}
              </button>
            </div>
          </div>
        ) : ask?.kind === "grbl" ? (
          <GrblDeviceForm form={ask.form} busy={busy} onChange={(form) => setAsk({ kind: "grbl", form })} onCancel={() => setAsk(null)} onAdd={() => void addGrbl(ask.form)} />
        ) : ask?.kind === "ruida" ? (
          <RuidaDeviceForm form={ask.form} busy={busy} onChange={(form) => setAsk({ kind: "ruida", form })} onCancel={() => setAsk(null)} onAdd={() => void addRuida(ask.form)} />
        ) : (
          <>
            {/* The ways to add a device are one menu, and on the phone the selected device's other actions another,
                so the footer is one row of the primary actions (it was eight buttons in three rows there). */}
            <MenuButton className="btn" label={t("dv.addDevice")} caret disabled={busy} items={addItems} placement="above" />
            {phone ? (
              <MenuButton className="btn" label={t("dv.more")} caret disabled={busy || !device} items={deviceItems} placement="above" />
            ) : (
              <>
                <button className="btn" disabled={busy || !device} title={t("dv.editACopyOfTheSelected")} onClick={() => device && void duplicate(device)}>
                  {t("dv.duplicate")}
                </button>
                <button className="btn" disabled={busy || !device} title={t("dv.deviceSettingsFieldRedDotGalvo")} onClick={() => setEditing(true)}>
                  {t("dv.edit")}
                </button>
                <button className="btn" disabled={busy || removeBlock !== null} title={removeBlock ?? t("dv.removeTheSelectedDevice")} onClick={() => device && setAsk({ kind: "remove", device })}>
                  {t("dv.remove")}
                </button>
              </>
            )}
            <button
              className="btn btn-primary"
              disabled={busy || activateBlock !== null}
              title={activateBlock ?? t("dv.workWithTheSelectedDevice")}
              onClick={() => device && setAsk({ kind: "switch", device })}
            >
              {t("dv.makeActive")}
            </button>
            <button className="btn" onClick={p.onClose}>
              {t("dv.close")}
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
        <p className="muted">{t("dv.loading")}</p>
      ) : list.devices.length === 0 ? (
        <p className="muted">{t("dv.noDevicesYetImportThemFrom")}</p>
      ) : (
        <div className="dev-table-wrap">
          <table className="dev-table" role="grid" aria-label={t("dv.devices")}>
            <thead>
              <tr>
                <th scope="col">{t("dv.name")}</th>
                <th scope="col">{t("dv.controller")}</th>
                <th scope="col">{t("dv.size")}</th>
                <th scope="col">{t("dv.origin")}</th>
                <th scope="col">{t("dv.source")}</th>
              </tr>
            </thead>
            {groups.map((g) => (
            <tbody key={g.kind}>
              {groups.length > 1 && (
                <tr className="dev-group-row">
                  <th scope="rowgroup" colSpan={5}>
                    {g.label} <span className="dev-sub">({g.devices.length})</span>
                  </th>
                </tr>
              )}
              {g.devices.map((d) => (
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
                        <span className="dev-star" title={t("dv.activeDevice")} aria-label={t("dv.activeDevice")}>
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
                  <td>{deviceSizeText(d)}</td>
                  <td>{originLabel(d.origin)}</td>
                  <td>{sourceLabel(d.source)}</td>
                </tr>
              ))}
            </tbody>
            ))}
          </table>
        </div>
      )}
      <input
        ref={markcfgInput}
        type="file"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) void importMarkcfg(f);
        }}
      />
      {device && <DeviceSettings device={device} details={details && details.id === device.id ? details : null} />}
      {editing && device && (
        <DeviceEditor
          device={device}
          active={device.id === list?.active}
          doc={p.doc ?? null}
          run={p.run ?? (() => Promise.reject(new Error("unavailable")))}
          notify={p.notify}
          report={p.report}
          onChanged={(id) => {
            void p.devices.refresh().then(() => id && setSelected(id));
            if (device.id === list?.active) p.onSwitched();
          }}
          onClose={() => setEditing(false)}
        />
      )}
      <p className="hint">{t("dv.importHint")}</p>
    </Modal>
  );
}

function noopClose() {}

/** The read-only summary of the selected device (LightBurn 2.x shows one under the list). */
function DeviceSettings(p: { device: DeviceSummary; details: { d: DeviceDetails | null; error: string | null } | null }) {
  const t = useT();
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
        <dt>{t("dv.controller")}</dt>
        <dd>{device.name}</dd>
        <dt>{t("dv.connection")}</dt>
        <dd>{device.lightburn_type ?? "–"}</dd>
        <dt>{t("dv.kind")}</dt>
        <dd>{kindLabel(device.kind)}</dd>
        <dt>{isGalvo(device.kind) ? t("rest.field") : t("rest.workspace")}</dt>
        <dd>{deviceSizeText(device)}</dd>
        <dt>{t("dv.origin")}</dt>
        <dd>{originLabel(device.origin)}</dd>
        <dt>{t("dv.source")}</dt>
        <dd>{sourceLabel(device.source)}</dd>
        <dt>{t("dv.runsJobs")}</dt>
        <dd>{device.controllable ? t("rest.yes") : t("dv.noDesignOnly")}</dd>
        {d?.base && (
          <>
            <dt>{t("dv.profileBase")}</dt>
            <dd>{d.base}</dd>
          </>
        )}
      </dl>
      {designOnly && <p className="run-hint">{designOnly}</p>}
      {p.details === null ? (
        <p className="muted">{t("dv.loadingSettings")}</p>
      ) : p.details.error ? (
        <p className="form-error">{p.details.error}</p>
      ) : (
        d && (
          <>
            {d.profile.length > 0 && (
              <details className="dev-group">
                <summary>{t("dv.openetchProfile")}</summary>
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
                  {d.diffs.length === 1
                    ? t("dv.oneDifference", { base: d.base ?? t("dv.theBaseProfile") })
                    : t("dv.nDifferences", { n: d.diffs.length, base: d.base ?? t("dv.theBaseProfile") })}
                </summary>
                <table className="lb-diff-table">
                  <thead>
                    <tr>
                      <th scope="col">{t("dv.setting")}</th>
                      <th scope="col">{t("dv.base")}</th>
                      <th scope="col">{t("dv.thisDevice")}</th>
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
              device.controllable && <p className="hint">{t("dv.profileMatches", { base: d.base ?? t("dv.itsBase") })}</p>
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
