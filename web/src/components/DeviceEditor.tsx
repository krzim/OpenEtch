// LightBurn's Device Settings for a galvo (Devices window > Edit), on the engine's `PUT /api/devices/{id}`: every
// profile field grouped like LightBurn's pages (Galvo and Basic Settings: field, red dot, galvo configuration, per
// galvo corrections, jump and delay defaults; Ports and Laser Settings: laser source and IO ports; the extension axis
// for a Z lift or rotary), plus the lens correction file (load / clear), the Calibrate Galvo Lens wizard, `.lbdev`
// export and the controller info. Only edited fields are sent (a JSON merge patch); the engine refuses raised limits
// and invalid values, and its message is shown at the field it names. The built-in device is read-only: duplicate it.

import { useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type Invocation } from "../api/client.ts";
import {
  checkValues,
  errorField,
  merged,
  profileGroups,
  profilePatch,
  profileValues,
  valueText,
  type FieldSpec,
  type ProfileValues,
} from "../api/deviceProfile.ts";
import { jobApi, saveBlob } from "../api/jobApi.ts";
import type { DeviceSummary } from "../api/devices.ts";
import type { GDocument } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { LensWizard } from "./LensWizard.tsx";
import { Modal, NumberField } from "./ui.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

type Tab = "galvo" | "ports" | "axis" | "lens" | "info";

const TAB_GROUPS: Record<Tab, string[]> = {
  galvo: ["general", "field", "reddot", "galvo", "galvo1", "galvo2", "jump", "delay"],
  ports: ["laser", "ports"],
  axis: ["axis"],
  lens: [],
  info: [],
};

interface Loaded {
  values: ProfileValues;
  /** The profile's own hard limits (they can only go down). */
  limits: ProfileValues;
  raw: Record<string, unknown> | null;
}

export function DeviceEditor(p: {
  device: DeviceSummary;
  /** It is the active device (lens calibration, controller info and the reported laser type need it). */
  active: boolean;
  doc: GDocument | null;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  notify: Notify;
  report: (e: unknown) => void;
  /** After a save, COR change or duplicate: refetch the device list (and select `id` when given). */
  onChanged: (id?: string) => void;
  onClose: () => void;
}) {
  const t = useT();
  const { device } = p;
  const readOnly = device.source === "builtin";
  const [tab, setTab] = useState<Tab>("galvo");
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [edits, setEdits] = useState<ProfileValues>({});
  const [name, setName] = useState(device.display_name);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [general, setGeneral] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [seq, setSeq] = useState(0);
  const [info, setInfo] = useState<Record<string, unknown> | null>(null);
  const [wizard, setWizard] = useState(false);
  const corInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let alive = true;
    setLoadError(null);
    (async () => {
      const raw = (await api.device(device.id)) as Record<string, unknown>;
      const lbdev = await jobApi.deviceLbdevText(device.id).catch(() => null);
      const inf = p.active ? await jobApi.machineInfo().catch(() => null) : null;
      if (!alive) return;
      const values = profileValues(raw?.profile, lbdev, inf);
      const limits: ProfileValues = {};
      for (const k of ["max_power_pct", "max_jump_speed_mm_s", "max_delay_us", "mark_speed.max_mm_s", "frequency_khz_max", "frequency_khz_min"]) {
        if (typeof values[k] === "number") limits[k] = values[k];
      }
      setLoaded({ values, limits, raw });
      setInfo(inf && typeof inf === "object" ? (inf as Record<string, unknown>) : null);
    })().catch((e: unknown) => alive && setLoadError(e instanceof ApiError ? (e.detail ?? e.message) : String(e)));
    return () => {
      alive = false;
    };
  }, [device.id, p.active, seq]);

  const values = useMemo(() => (loaded ? merged(loaded.values, edits) : {}), [loaded, edits]);
  const clientErrors = useMemo(() => (loaded ? checkValues(values, loaded.limits) : {}), [values, loaded]);
  const patch = loaded ? profilePatch(loaded.values, edits) : null;
  const renamed = name.trim() !== "" && name.trim() !== device.display_name;
  const dirty = patch !== null || renamed;
  const hasClientErrors = Object.keys(clientErrors).length > 0;
  const swap = values.swap_xy === true;
  const groups = profileGroups(swap);

  const edit = (path: string, v: unknown) => {
    setEdits((e) => ({ ...e, [path]: v }));
    setErrors((x) => {
      const { [path]: _, ...rest } = x;
      return rest;
    });
    setGeneral(null);
  };

  const save = async () => {
    if (!dirty || hasClientErrors || readOnly) return;
    setBusy(true);
    setGeneral(null);
    try {
      const res = (await jobApi.deviceEdit(device.id, { ...(patch ? { profile: patch } : {}), ...(renamed ? { display_name: name.trim() } : {}) })) as {
        warnings?: unknown;
        reactivated?: boolean;
      };
      const warnings = Array.isArray(res?.warnings) ? res.warnings.map(String) : [];
      p.notify(warnings.length ? `${t("dev.saved")} ${warnings.join("; ")}` : res?.reactivated ? t("dev.savedReactivated") : t("dev.saved"), warnings.length ? "error" : "info");
      setEdits({});
      setSeq((s) => s + 1);
      p.onChanged();
    } catch (e) {
      const msg = e instanceof ApiError ? (e.detail ?? e.message) : String(e);
      const f = errorField(msg);
      if (f) {
        setErrors({ [f]: msg });
        // Show the page with the field.
        const g = profileGroups(swap).find((x) => x.fields.some((y) => y.path === f));
        const tb = (Object.keys(TAB_GROUPS) as Tab[]).find((k) => g && TAB_GROUPS[k].includes(g.id));
        if (tb) setTab(tb);
      } else setGeneral(msg);
    } finally {
      setBusy(false);
    }
  };

  const duplicate = () => {
    setBusy(true);
    jobApi
      .deviceDuplicate(device.id)
      .then((r) => {
        const id = (r as { device?: { id?: string } })?.device?.id;
        p.notify(t("dev.duplicated"));
        p.onChanged(id);
        p.onClose();
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };

  const exportLbdev = () =>
    void jobApi
      .deviceLbdev(device.id)
      .then(({ blob, filename }) => saveBlob(blob, filename ?? `${device.display_name}.lbdev`))
      .catch(p.report);

  const corAction = (fn: () => Promise<unknown>, ok: string) => {
    setBusy(true);
    fn()
      .then((r) => {
        const w = Array.isArray((r as { warnings?: unknown })?.warnings) ? ((r as { warnings: unknown[] }).warnings.map(String) as string[]) : [];
        p.notify(w.length ? `${ok} ${w.join("; ")}` : ok, w.length ? "error" : "info");
        setSeq((s) => s + 1);
        p.onChanged();
      })
      .catch(p.report)
      .finally(() => setBusy(false));
  };

  const tabs: { v: Tab; label: string }[] = [
    { v: "galvo", label: t("dev.tabGalvo") },
    { v: "ports", label: t("dev.tabPorts") },
    { v: "axis", label: t("dev.tabAxis") },
    { v: "lens", label: t("dev.tabLens") },
    { v: "info", label: t("dev.tabInfo") },
  ];
  const corFile = loaded ? values.cor_file : undefined;

  return (
    <Modal
      title={t("dev.title", { name: device.display_name })}
      wide
      // While the wizard is open, Escape closes only the wizard (every Modal listens on the window).
      onClose={wizard ? noop : p.onClose}
      footer={
        <>
          <button className="btn" onClick={exportLbdev} title={t("dev.exportTitle")}>
            {t("dev.export")}
          </button>
          {readOnly ? (
            <button className="btn btn-primary" disabled={busy} onClick={duplicate}>
              {t("dev.duplicateToEdit")}
            </button>
          ) : (
            <>
              <button className="btn" disabled={!dirty || busy} onClick={() => (setEdits({}), setName(device.display_name), setErrors({}), setGeneral(null))}>
                {t("dev.revert")}
              </button>
              <button className="btn btn-primary" disabled={!dirty || busy || hasClientErrors} title={hasClientErrors ? t("dev.fixErrors") : undefined} onClick={() => void save()}>
                {busy ? t("dev.saving") : t("dev.save")}
              </button>
            </>
          )}
          <button className="btn" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      {readOnly && <p className="run-hint">{t("dev.builtinReadOnly")}</p>}
      {!device.controllable && <p className="run-hint">{t("dev.designOnly")}</p>}
      <label className="field">
        <span className="field-label">{t("dev.displayName")}</span>
        <span className="field-input">
          <input type="text" value={name} disabled={readOnly} onChange={(e) => setName(e.target.value)} />
        </span>
      </label>
      <div className="segmented dev-tabs" role="tablist" aria-label={t("dev.pages")}>
        {tabs.map((x) => (
          <button key={x.v} role="tab" aria-selected={tab === x.v} className={tab === x.v ? "active" : ""} onClick={() => setTab(x.v)}>
            {x.label}
          </button>
        ))}
      </div>
      {general && (
        <p className="form-error" role="alert">
          {general}
        </p>
      )}
      {loadError ? (
        <p className="form-error">{loadError}</p>
      ) : !loaded ? (
        <p className="muted">{t("common.loading")}</p>
      ) : tab === "lens" ? (
        <div className="dev-page">
          <h4>{t("dev.corTitle")}</h4>
          <dl className="kv">
            <dt>{t("dev.corFile")}</dt>
            <dd>{corFile ? String(corFile) : t("dev.corNone")}</dd>
          </dl>
          <input
            ref={corInput}
            type="file"
            accept=".cor"
            hidden
            onChange={(e) => {
              const f = e.target.files?.[0];
              e.target.value = "";
              if (f) corAction(() => jobApi.corUpload(device.id, f, f.name), t("dev.corLoaded", { name: f.name }));
            }}
          />
          <div className="et-dlg-row">
            <button className="btn" disabled={readOnly || busy} onClick={() => corInput.current?.click()}>
              {t("dev.corLoad")}
            </button>
            <button className="btn" disabled={readOnly || busy || !corFile} onClick={() => corAction(() => jobApi.corClear(device.id), t("dev.corCleared"))}>
              {t("dev.corClear")}
            </button>
          </div>
          <p className="hint">{t("dev.corHint")}</p>
          <h4>{t("lens.title")}</h4>
          <p className="hint">{t("lens.intro")}</p>
          <button
            className="btn"
            disabled={!p.active || !device.controllable}
            title={!p.active ? t("lens.activeOnly") : undefined}
            onClick={() => setWizard(true)}
          >
            {t("lens.open")}
          </button>
          <Group title={t("dev.currentCorrections")} fields={[...groups.find((g) => g.id === "field")!.fields.slice(1), ...groups.find((g) => g.id === "galvo1")!.fields, ...groups.find((g) => g.id === "galvo2")!.fields]} values={values} readOnly />
        </div>
      ) : tab === "info" ? (
        <ControllerInfo active={p.active} info={info} onRefresh={() => setSeq((s) => s + 1)} />
      ) : (
        <div className="dev-page">
          {groups
            .filter((g) => TAB_GROUPS[tab].includes(g.id))
            .map((g) => (
              <Group
                key={g.id}
                title={g.id === "galvo1" || g.id === "galvo2" ? `${g.title} (${g.fields[0].label.slice(-2, -1)})` : g.title}
                fields={g.id === "axis" && values["axis.enabled"] !== true ? g.fields.slice(0, 1) : g.id === "axis" && values["axis.role"] === "rotary" ? g.fields.filter((f) => !/min_mm|max_mm|steps_per_mm/.test(f.path)) : g.fields}
                values={values}
                edited={edits}
                errors={{ ...clientErrors, ...errors }}
                limits={loaded.limits}
                readOnly={readOnly}
                onEdit={edit}
              />
            ))}
          {tab === "ports" && <p className="hint">{t("dev.polarity")}</p>}
          {tab === "axis" && <p className="hint">{t("dev.axisHint")}</p>}
        </div>
      )}
      {wizard && (
        <LensWizard
          doc={p.doc}
          device={device}
          notify={p.notify}
          report={p.report}
          onApplied={() => {
            setSeq((s) => s + 1);
            p.onChanged();
          }}
          onClose={() => setWizard(false)}
        />
      )}
    </Modal>
  );
}

function noop() {}

/** One group of fields (a LightBurn section). */
function Group(p: {
  title: string;
  fields: FieldSpec[];
  values: ProfileValues;
  edited?: ProfileValues;
  errors?: Record<string, string>;
  limits?: ProfileValues;
  readOnly?: boolean;
  onEdit?: (path: string, v: unknown) => void;
}) {
  return (
    <fieldset className="job-fieldset dev-group-edit">
      <legend>{p.title}</legend>
      {p.fields.map((f) => (
        <FieldRow
          key={f.path}
          f={f}
          value={p.values[f.path]}
          edited={p.edited ? f.path in p.edited : false}
          error={p.errors?.[f.path] ?? null}
          limit={p.limits?.[f.path]}
          readOnly={!!p.readOnly || !!f.readOnly || !p.onEdit}
          onEdit={(v) => p.onEdit?.(f.path, v)}
        />
      ))}
    </fieldset>
  );
}

function FieldRow(p: { f: FieldSpec; value: unknown; edited: boolean; error: string | null; limit: unknown; readOnly: boolean; onEdit: (v: unknown) => void }) {
  const t = useT();
  const { f } = p;
  const unknown = p.value === undefined;
  const title = [f.hint, f.lb ? `LightBurn: ${f.lb}` : null, f.limit && typeof p.limit === "number" ? t("dev.limitHint", { limit: p.limit }) : null, unknown ? t("dev.notReported") : null]
    .filter(Boolean)
    .join(" · ");
  let input;
  if (p.readOnly && f.kind !== "number") {
    input = (
      <dl className="kv dev-ro">
        <dt>{f.label}</dt>
        <dd>{f.kind === "select" ? (f.options?.find((o) => o.value === p.value)?.label ?? valueText(p.value)) : valueText(p.value)}</dd>
      </dl>
    );
  } else if (f.kind === "number") {
    input = (
      <NumberField
        label={f.label}
        unit={f.unit}
        value={typeof p.value === "number" ? p.value : null}
        min={f.min}
        max={f.max}
        digits={f.digits}
        placeholder={unknown ? t("dev.notReportedShort") : undefined}
        disabled={p.readOnly}
        onCommit={(v) => v !== null && p.onEdit(v)}
      />
    );
  } else if (f.kind === "bool") {
    input = (
      <label className="field field-check">
        <span className="field-label">{f.label}</span>
        {unknown && !p.edited ? (
          <select className="dev-tri" value="" onChange={(e) => p.onEdit(e.target.value === "on")} aria-label={f.label}>
            <option value="">{t("dev.notReportedShort")}</option>
            <option value="on">{t("frame.on")}</option>
            <option value="off">{t("frame.off")}</option>
          </select>
        ) : (
          <input type="checkbox" checked={p.value === true} onChange={(e) => p.onEdit(e.target.checked)} />
        )}
      </label>
    );
  } else if (f.kind === "select") {
    input = (
      <label className="field">
        <span className="field-label">{f.label}</span>
        <span className="field-input">
          <select value={typeof p.value === "string" ? p.value : ""} onChange={(e) => e.target.value && p.onEdit(e.target.value)}>
            {unknown && <option value="">{t("dev.notReportedShort")}</option>}
            {f.options?.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </span>
      </label>
    );
  } else if (f.kind === "bit") {
    const lo = f.minBit ?? 0;
    input = (
      <label className="field">
        <span className="field-label">{f.label}</span>
        <span className="field-input">
          <select value={unknown ? "?" : p.value === null ? "" : String(p.value)} onChange={(e) => e.target.value !== "?" && p.onEdit(e.target.value === "" ? null : Number(e.target.value))}>
            {unknown && <option value="?">{t("dev.notReportedShort")}</option>}
            <option value="">{t("dev.none")}</option>
            {Array.from({ length: 16 - lo }, (_, i) => i + lo).map((b) => (
              <option key={b} value={b}>
                {t("dev.bit", { bit: b })}
              </option>
            ))}
          </select>
        </span>
      </label>
    );
  } else {
    input = (
      <label className="field">
        <span className="field-label">{f.label}</span>
        <span className="field-input">
          <input type="text" value={typeof p.value === "string" ? p.value : ""} placeholder={unknown ? t("dev.notReportedShort") : undefined} onChange={(e) => p.onEdit(e.target.value)} />
        </span>
      </label>
    );
  }
  return (
    <div className={`dev-field${p.edited ? " edited" : ""}${p.error ? " invalid" : ""}`} title={title || undefined}>
      {input}
      {p.error && (
        <p className="form-error dev-field-error" role="alert">
          {p.error}
        </p>
      )}
    </div>
  );
}

/** LightBurn's Get Controller Info: GetSerialNo / GetVersion words (not decoded), the source and the axis. */
function ControllerInfo(p: { active: boolean; info: Record<string, unknown> | null; onRefresh: () => void }) {
  const t = useT();
  if (!p.active) return <p className="hint">{t("dev.infoActiveOnly")}</p>;
  const i = p.info;
  if (!i) return <p className="muted">{t("dev.infoNone")}</p>;
  const words = (v: unknown) => (Array.isArray(v) ? v.map((w) => (typeof w === "number" ? `0x${w.toString(16).padStart(4, "0")}` : String(w))).join(" ") : "–");
  const ax = i.axis && typeof i.axis === "object" ? (i.axis as Record<string, unknown>) : null;
  return (
    <div className="dev-page">
      <dl className="kv">
        <dt>{t("dev.infoConnected")}</dt>
        <dd>{i.connected ? t("frame.on") : t("frame.off")}</dd>
        <dt>{t("dev.infoProfile")}</dt>
        <dd>{String(i.profile ?? "–")}</dd>
        <dt>{t("dev.infoSource")}</dt>
        <dd>{String(i.laser_source || "–")}</dd>
        <dt>{t("dev.infoKind")}</dt>
        <dd>
          {String(i.laser_kind ?? "–")} · {String(i.fiber_kind ?? "–")}
        </dd>
        <dt>{t("dev.infoSerial")}</dt>
        <dd className="mono">{typeof i.serial_hex === "string" ? i.serial_hex : "–"}</dd>
        <dt>{t("dev.infoSerialWords")}</dt>
        <dd className="mono">{words(i.serial_words)}</dd>
        <dt>{t("dev.infoVersion")}</dt>
        <dd className="mono">{words(i.version_words)}</dd>
        <dt>{t("dev.infoAxis")}</dt>
        <dd>{ax ? `${String(ax.role ?? "z")}${typeof ax.steps_per_mm === "number" ? ` · ${ax.steps_per_mm} steps/mm` : ""}` : t("dev.none")}</dd>
      </dl>
      <p className="hint">{t("dev.infoHint")}</p>
      <button className="btn small" onClick={p.onRefresh}>
        {t("dev.infoRefresh")}
      </button>
    </div>
  );
}
