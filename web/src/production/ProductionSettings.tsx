// Settings > Notifications, Maintenance and Extractor (engine `production::notify`, `meter`, `extractor`), as sections of
// the Preferences dialog. Notification targets (ntfy, a webhook, Home Assistant) pick their events and have a Test
// button; their tokens are written, never read back. Maintenance reminders count marking hours or runs on the active
// device. The extractor is switched through the board's Busy output, a Pi GPIO pin, Home Assistant or a webhook, with a
// run-on and an optional Start gate. Every save is a UI-only engine call; nothing here can start a burn.

import { useEffect, useState } from "react";
import { errorText } from "../api/errorText.ts";
import { Icon, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { MsgKey } from "../workspace/i18n.ts";
import { prodApi } from "./api.ts";
import {
  DEFAULT_EXTRACTOR,
  EVENT_KINDS,
  draftOf,
  extractorProblem,
  hours,
  newTarget,
  reminderLeft,
  reminderProblem,
  targetProblem,
  type ExtractorConfig,
  type ExtractorStatus,
  type MeterStatus,
  type ReminderDraft,
  type TargetDraft,
  type TargetKind,
  type TargetView,
} from "./model.ts";
import { refreshProduction } from "./store.ts";
import "./production.css";

type Report = (e: unknown) => void;

export function ProductionSettingsSections(p: { report?: Report }) {
  const report = p.report ?? (() => undefined);
  return (
    <>
      <NotificationsSection report={report} />
      <MaintenanceSection report={report} />
      <ExtractorSection report={report} />
    </>
  );
}

function useStatusLine(): [string | null, (s: string | null) => void] {
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    if (!msg) return;
    const id = window.setTimeout(() => setMsg(null), 6000);
    return () => window.clearTimeout(id);
  }, [msg]);
  return [msg, setMsg];
}

// ---- notifications --------------------------------------------------------------------------------------------------

function NotificationsSection({ report }: { report: Report }) {
  const t = useT();
  const [views, setViews] = useState<TargetView[] | null>(null);
  const [drafts, setDrafts] = useState<TargetDraft[]>([]);
  const [delay, setDelay] = useState(30);
  const [dirty, setDirty] = useState(false);
  const [msg, setMsg] = useStatusLine();
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    prodApi
      .notifications()
      .then((v) => {
        const x = v as { targets: TargetView[]; waiting_delay_s: number };
        setViews(x.targets);
        setDrafts(x.targets.map(draftOf));
        setDelay(x.waiting_delay_s);
        setDirty(false);
      })
      .catch((e) => {
        setViews([]);
        setError(errorText(e));
      });
  useEffect(() => void load(), []);

  const edit = (i: number, patch: Partial<TargetDraft>) => {
    setDrafts((d) => d.map((x, k) => (k === i ? { ...x, ...patch } : x)));
    setDirty(true);
  };
  const tokenSet = (d: TargetDraft) => !!views?.find((v) => v.id === d.id)?.token_set;
  const problem = drafts.map((d) => targetProblem(d, tokenSet(d))).find((x) => x !== null) ?? null;
  const save = () =>
    prodApi
      .setNotifications(drafts, delay)
      .then(() => {
        setMsg(t("prod.saved"));
        setError(null);
        void load();
        void refreshProduction();
      })
      .catch((e) => setError(errorText(e)));
  const test = (id: string) =>
    prodApi
      .testTarget(id)
      .then((r) => setMsg(t("prod.tgTestOk", { result: r.result ?? "" })))
      .catch((e) => setError(t("prod.tgTestFailed", { error: errorText(e) })));

  return (
    <section className="prefs-section prod-section">
      <h4>
        <Icon name="bell" size={16} /> {t("prod.notifications")}
      </h4>
      <p className="hint">{t("prod.notificationsHint")}</p>
      {views === null && <p className="muted">{t("common.loading")}</p>}
      {drafts.map((d, i) => {
        const view = views?.find((v) => v.id === d.id);
        return (
          <fieldset key={d.id ?? `new-${i}`} className="prod-target">
            <legend>
              <input
                className="prod-name"
                aria-label={t("prod.tgName")}
                value={d.name}
                maxLength={60}
                onChange={(e) => edit(i, { name: e.target.value })}
              />
            </legend>
            <div className="prod-row">
              <label>
                {t("prod.tgKind")}
                <select value={d.kind} onChange={(e) => edit(i, { kind: e.target.value as TargetKind })}>
                  <option value="ntfy">ntfy</option>
                  <option value="webhook">{t("prod.tgWebhook")}</option>
                  <option value="home_assistant">Home Assistant</option>
                </select>
              </label>
              <label className="prod-check">
                <input type="checkbox" checked={d.enabled} onChange={(e) => edit(i, { enabled: e.target.checked })} />
                {t("prod.tgEnabled")}
              </label>
            </div>
            <label className="prod-wide">
              {d.kind === "ntfy" ? t("prod.tgServer") : d.kind === "home_assistant" ? t("prod.tgHaUrl") : t("prod.tgUrl")}
              <input type="url" inputMode="url" spellCheck={false} value={d.url} onChange={(e) => edit(i, { url: e.target.value })} />
            </label>
            {d.kind === "ntfy" && (
              <label className="prod-wide">
                {t("prod.tgTopic")}
                <input spellCheck={false} value={d.topic ?? ""} onChange={(e) => edit(i, { topic: e.target.value })} />
              </label>
            )}
            <label className="prod-wide">
              {d.kind === "home_assistant" ? t("prod.tgHaToken") : t("prod.tgToken")}
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder={view?.token_set ? t("prod.tgTokenKept") : t("prod.tgTokenNone")}
                value={d.token ?? ""}
                onChange={(e) => edit(i, { token: e.target.value })}
              />
            </label>
            <div className="prod-events" role="group" aria-label={t("prod.tgEvents")}>
              {EVENT_KINDS.map((k) => (
                <label key={k} className="prod-check">
                  <input
                    type="checkbox"
                    checked={d.events.includes(k)}
                    onChange={(e) => edit(i, { events: e.target.checked ? [...d.events, k] : d.events.filter((x) => x !== k) })}
                  />
                  {t(`prod.ev.${k}` as MsgKey)}
                </label>
              ))}
            </div>
            <div className="prod-row">
              <button className="btn small" disabled={!d.id || dirty} title={dirty ? t("prod.tgSaveFirst") : undefined} onClick={() => d.id && void test(d.id)}>
                {t("prod.tgTest")}
              </button>
              {view?.last && (
                <span className={`hint ${view.last.ok ? "" : "prod-bad"}`}>
                  {t("prod.tgLast", { result: view.last.result, time: new Date(view.last.time_unix_s * 1000).toLocaleTimeString() })}
                </span>
              )}
              <button
                className="btn small btn-danger prod-end"
                onClick={() => {
                  setDrafts((x) => x.filter((_, k) => k !== i));
                  setDirty(true);
                }}
              >
                {t("prod.remove")}
              </button>
            </div>
          </fieldset>
        );
      })}
      <div className="prod-row">
        <select
          aria-label={t("prod.tgAdd")}
          value=""
          onChange={(e) => {
            if (!e.target.value) return;
            setDrafts((d) => [...d, newTarget(e.target.value as TargetKind, d.length + 1)]);
            setDirty(true);
          }}
        >
          <option value="">{t("prod.tgAdd")}</option>
          <option value="ntfy">ntfy</option>
          <option value="webhook">{t("prod.tgWebhook")}</option>
          <option value="home_assistant">Home Assistant</option>
        </select>
        <NumberField
          label={t("prod.waitingDelay")}
          unit="s"
          value={delay}
          min={0}
          max={3600}
          digits={0}
          onCommit={(v) => {
            if (v === null) return;
            setDelay(Math.round(v));
            setDirty(true);
          }}
        />
      </div>
      {(problem || error || msg) && (
        <p className={problem || error ? "run-hint" : "hint"} role={problem || error ? "alert" : "status"}>
          {problem ?? error ?? msg}
        </p>
      )}
      <div className="prod-row">
        <button className="btn btn-primary" disabled={!dirty || !!problem} onClick={() => void save().catch(report)}>
          {t("prod.save")}
        </button>
        {dirty && (
          <button className="btn" onClick={() => void load()}>
            {t("prod.revert")}
          </button>
        )}
      </div>
    </section>
  );
}

// ---- maintenance ----------------------------------------------------------------------------------------------------

function MaintenanceSection({ report }: { report: Report }) {
  const t = useT();
  const [m, setM] = useState<MeterStatus | null>(null);
  const [drafts, setDrafts] = useState<ReminderDraft[]>([]);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useStatusLine();
  const apply = (v: unknown) => {
    const x = v as MeterStatus;
    setM(x);
    setDrafts(x.reminders.map((r) => ({ id: r.id, name: r.name, every_hours: r.every_hours, every_runs: r.every_runs })));
    setDirty(false);
  };
  const load = () => prodApi.meter().then(apply).catch((e) => setError(errorText(e)));
  useEffect(() => void load(), []);
  const edit = (i: number, patch: Partial<ReminderDraft>) => {
    setDrafts((d) => d.map((x, k) => (k === i ? { ...x, ...patch } : x)));
    setDirty(true);
  };
  const problem = drafts.map(reminderProblem).find((x) => x !== null) ?? null;
  const save = () =>
    prodApi
      .setReminders(drafts)
      .then((v) => {
        apply(v);
        setMsg(t("prod.saved"));
        setError(null);
        void refreshProduction();
      })
      .catch((e) => setError(errorText(e)));
  const done = (id: string) =>
    prodApi
      .reminderDone(id)
      .then((v) => {
        apply(v);
        void refreshProduction();
      })
      .catch(report);
  return (
    <section className="prefs-section prod-section">
      <h4>
        <Icon name="wrench" size={16} /> {t("prod.maintenance")}
      </h4>
      {m && (
        <p className="hint">
          {t("prod.meterLine", { hours: hours(m.hours), measured: hours(m.measured_hours), runs: m.runs })}
        </p>
      )}
      <p className="hint">{t("prod.meterHint")}</p>
      {drafts.map((r, i) => {
        const live = m?.reminders.find((x) => x.id === r.id);
        return (
          <div key={r.id ?? `new-${i}`} className="prod-reminder">
            <input aria-label={t("prod.remName")} placeholder={t("prod.remNamePlaceholder")} value={r.name} maxLength={80} onChange={(e) => edit(i, { name: e.target.value })} />
            <NumberField label={t("prod.remEveryHours")} unit="h" value={r.every_hours ?? null} allowEmpty min={0.1} max={100000} digits={1} onCommit={(v) => edit(i, { every_hours: v })} />
            <NumberField label={t("prod.remEveryRuns")} value={r.every_runs ?? null} allowEmpty min={1} max={10000000} digits={0} onCommit={(v) => edit(i, { every_runs: v === null ? null : Math.round(v) })} />
            {live && <span className={live.due ? "prod-due" : "hint"}>{reminderLeft(live)}</span>}
            {live && (
              <button className="btn small" disabled={dirty} onClick={() => void done(live.id)}>
                {t("prod.remMarkDone")}
              </button>
            )}
            <button
              className="btn small btn-danger"
              onClick={() => {
                setDrafts((d) => d.filter((_, k) => k !== i));
                setDirty(true);
              }}
            >
              {t("prod.remove")}
            </button>
          </div>
        );
      })}
      <div className="prod-row">
        <button
          className="btn"
          onClick={() => {
            setDrafts((d) => [...d, { name: t("prod.remDefaultName"), every_hours: 40, every_runs: null }]);
            setDirty(true);
          }}
        >
          {t("prod.remAdd")}
        </button>
        <button className="btn btn-primary" disabled={!dirty || !!problem} onClick={() => void save()}>
          {t("prod.save")}
        </button>
      </div>
      {(problem || error || msg) && (
        <p className={problem || error ? "run-hint" : "hint"} role={problem || error ? "alert" : "status"}>
          {problem ?? error ?? msg}
        </p>
      )}
    </section>
  );
}

// ---- extractor ------------------------------------------------------------------------------------------------------

function ExtractorSection({ report }: { report: Report }) {
  const t = useT();
  const [st, setSt] = useState<ExtractorStatus | null>(null);
  const [c, setC] = useState<ExtractorConfig>(DEFAULT_EXTRACTOR);
  const [targets, setTargets] = useState<TargetView[]>([]);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useStatusLine();
  const apply = (v: unknown) => {
    const x = v as ExtractorStatus;
    setSt(x);
    setC({ ...DEFAULT_EXTRACTOR, ...x.config });
    setDirty(false);
  };
  useEffect(() => {
    void prodApi.extractor().then(apply).catch((e) => setError(errorText(e)));
    void prodApi
      .notifications()
      .then((v) => setTargets((v as { targets: TargetView[] }).targets))
      .catch(() => undefined);
  }, []);
  const edit = (patch: Partial<ExtractorConfig>) => {
    setC((x) => ({ ...x, ...patch }));
    setDirty(true);
  };
  const problem = extractorProblem(c);
  const save = () =>
    prodApi
      .setExtractor(c)
      .then((v) => {
        apply(v);
        setMsg(t("prod.saved"));
        setError(null);
        void refreshProduction();
      })
      .catch((e) => setError(errorText(e)));
  const kind = c.output.kind;
  const haTargets = targets.filter((x) => x.kind === "home_assistant");
  const hookTargets = targets.filter((x) => x.kind === "webhook");
  return (
    <section className="prefs-section prod-section">
      <h4>
        <Icon name="fan" size={16} /> {t("prod.extractor")}
      </h4>
      <p className="hint">{t("prod.extractorHint")}</p>
      <label className="prod-wide">
        {t("prod.exOutput")}
        <select
          value={kind}
          onChange={(e) => {
            const k = e.target.value;
            const output: ExtractorConfig["output"] =
              k === "gpio"
                ? { kind: "gpio", pin: 17, active_low: false }
                : k === "home_assistant"
                  ? { kind: "home_assistant", target: haTargets[0]?.id ?? "", entity_id: "switch.extractor" }
                  : k === "webhook"
                    ? { kind: "webhook", target: hookTargets[0]?.id ?? "" }
                    : k === "lmc_busy"
                      ? { kind: "lmc_busy" }
                      : { kind: "none" };
            edit({ output, gate: output.kind === "none" && c.gate.kind === "switched_on" ? { kind: "none" } : c.gate });
          }}
        >
          <option value="none">{t("prod.exNone")}</option>
          <option value="lmc_busy">{t("prod.exBusy", { bit: st?.busy_output ?? "–" })}</option>
          <option value="gpio">{t("prod.exGpio")}</option>
          <option value="home_assistant">Home Assistant</option>
          <option value="webhook">{t("prod.tgWebhook")}</option>
        </select>
      </label>
      {kind === "lmc_busy" && st?.busy_output === null && <p className="run-hint">{t("prod.exNoBusy")}</p>}
      {c.output.kind === "gpio" && (
        <div className="prod-row">
          <NumberField label={t("prod.exPin")} value={c.output.pin} min={0} max={53} digits={0} onCommit={(v) => v !== null && edit({ output: { kind: "gpio", pin: Math.round(v), active_low: c.output.kind === "gpio" && !!c.output.active_low } })} />
          <label className="prod-check">
            <input type="checkbox" checked={!!c.output.active_low} onChange={(e) => c.output.kind === "gpio" && edit({ output: { ...c.output, active_low: e.target.checked } })} />
            {t("prod.exActiveLow")}
          </label>
        </div>
      )}
      {(c.output.kind === "home_assistant" || c.output.kind === "webhook") && (
        <label className="prod-wide">
          {t("prod.exTargetLabel")}
          <select
            value={c.output.target}
            onChange={(e) => (c.output.kind === "home_assistant" || c.output.kind === "webhook") && edit({ output: { ...c.output, target: e.target.value } })}
          >
            <option value="">{t("prod.exPickTarget")}</option>
            {(c.output.kind === "home_assistant" ? haTargets : hookTargets).map((x) => (
              <option key={x.id} value={x.id}>
                {x.name}
              </option>
            ))}
          </select>
        </label>
      )}
      {c.output.kind === "home_assistant" && (
        <label className="prod-wide">
          {t("prod.exEntityLabel")}
          <input spellCheck={false} value={c.output.entity_id} onChange={(e) => c.output.kind === "home_assistant" && edit({ output: { ...c.output, entity_id: e.target.value.trim() } })} />
        </label>
      )}
      {kind !== "none" && (
        <div className="prod-row">
          <label className="prod-check">
            <input type="checkbox" checked={c.auto} onChange={(e) => edit({ auto: e.target.checked })} />
            {t("prod.exAuto")}
          </label>
          <NumberField label={t("prod.exRunOnLabel")} unit="s" value={c.run_on_s} min={0} max={3600} digits={0} onCommit={(v) => v !== null && edit({ run_on_s: Math.round(v) })} />
        </div>
      )}
      <label className="prod-wide">
        {t("prod.exGate")}
        <select
          value={c.gate.kind}
          onChange={(e) => {
            const k = e.target.value;
            edit({ gate: k === "input" ? { kind: "input", bit: 6, active_low: false } : k === "switched_on" ? { kind: "switched_on" } : { kind: "none" } });
          }}
        >
          <option value="none">{t("prod.exGateNone")}</option>
          <option value="input">{t("prod.exGateInput")}</option>
          <option value="switched_on" disabled={kind === "none"}>
            {t("prod.exGateOn")}
          </option>
        </select>
      </label>
      {c.gate.kind === "input" && (
        <div className="prod-row">
          <NumberField label={t("prod.exInputBit")} value={c.gate.bit} min={0} max={15} digits={0} onCommit={(v) => v !== null && edit({ gate: { kind: "input", bit: Math.round(v), active_low: c.gate.kind === "input" && !!c.gate.active_low } })} />
          <label className="prod-check">
            <input type="checkbox" checked={!!c.gate.active_low} onChange={(e) => c.gate.kind === "input" && edit({ gate: { ...c.gate, active_low: e.target.checked } })} />
            {t("prod.exActiveLow")}
          </label>
        </div>
      )}
      {(problem || error || msg) && (
        <p className={problem || error ? "run-hint" : "hint"} role={problem || error ? "alert" : "status"}>
          {problem ?? error ?? msg}
        </p>
      )}
      <div className="prod-row">
        <button className="btn btn-primary" disabled={!dirty || !!problem} onClick={() => void save()}>
          {t("prod.save")}
        </button>
        {st?.configured && !dirty && (
          <button
            className="btn"
            onClick={() =>
              void prodApi
                .switchExtractor(!st.state.on)
                .then(apply)
                .catch(report)
            }
          >
            {st.state.on ? t("prod.exSwitchOff") : t("prod.exSwitchOn")}
          </button>
        )}
        {st?.state.last_error && <span className="run-hint">{st.state.last_error}</span>}
      </div>
    </section>
  );
}
