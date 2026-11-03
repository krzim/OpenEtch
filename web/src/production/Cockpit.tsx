// The production cockpit (short list #1): a full-screen operator view for the foot switch cycle, built for an iPad on
// a stand beside the laser (and usable on a phone or a desktop). It shows, large: what the machine is doing and
// whether it waits for the operator (a layer pause, the foot switch, the framed job), the countdown, done against the
// target (Stop At) with the time left, the values the next run marks per slot (tap a slot to mark its pocket empty),
// the last burn and cycle times, the extractor and maintenance due, and the last serial ledger runs with a re-mark per
// slot. Start is the app's one Start (laser/StartButton.tsx, with the Job Checklist inline when there is no PIN);
// pending runs (an agent's, a re-mark) are approved below it as in the Laser window.
//
// Safety: STOP is in the cockpit's own bar, always on screen, above everything the cockpit shows (class `btn-stop`,
// z-index --z-stop); Pause / Resume and Stop are never blocked. While the cockpit is open it holds the engine's design
// lock (renewed every 15 s, released when it closes; engine `production::lock`), so nobody (another device, an agent)
// edits the job the foot switch burns; the counter and empty slots stay editable here. Continue at a layer pause is the
// person's press, like the foot switch; nothing here bypasses `approve`.
//
// Opened from the Laser ribbon, the Production window, command search ("Production cockpit") or `#/cockpit`, which
// the installable app's shortcut opens directly.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { api, execute } from "../api/client.ts";
import { jobApi } from "../api/jobApi.ts";
import { noPinMode, stopToast } from "../api/runs.ts";
import { runHotkeyAction } from "../components/commands.ts";
import { JobCounter, useJobStatus } from "../components/JobControls.tsx";
import { PendingApprovals } from "../components/LaserPanel.tsx";
import { Icon } from "../components/ui.tsx";
import { StartButton } from "../laser/StartButton.tsx";
import { onlineOf } from "../laser/laserContext.ts";
import { useLaserLive } from "../laser/useLaserLive.ts";
import { useT } from "../workspace/usePrefs.ts";
import { prodApi } from "./api.ts";
import { LOCK_RENEW_MS, clock, cockpitPhase, counterText, hours, nextValues, shortDuration, slotLine, toggleEmpty, type ProductionStatus } from "./model.ts";
import { refreshProduction, useProduction } from "./store.ts";
import "./cockpit.css";

/** This browser's lock holder id (one per page load) and a short label for the others ("iPad", "Mac"). */
const HOLDER = `cockpit-${Math.random().toString(36).slice(2, 10)}`;

function deviceLabel(): string {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/iPad/.test(ua) || (/Macintosh/.test(ua) && typeof navigator !== "undefined" && navigator.maxTouchPoints > 1)) return "iPad";
  if (/iPhone/.test(ua)) return "iPhone";
  if (/Android/.test(ua)) return "Android";
  if (/Macintosh/.test(ua)) return "Mac";
  if (/Windows/.test(ua)) return "Windows";
  return "Linux";
}

/** Hold the design lock while mounted. */
function useCockpitLock(report: (e: unknown) => void): void {
  useEffect(() => {
    let alive = true;
    const label = deviceLabel();
    const renew = () => {
      if (alive) void prodApi.lock(true, HOLDER, label).then(() => refreshProduction(), report);
    };
    renew();
    const timer = window.setInterval(renew, LOCK_RENEW_MS);
    const release = () => void prodApi.lock(false, HOLDER, label).catch(() => undefined);
    window.addEventListener("pagehide", release);
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener("pagehide", release);
      release();
    };
  }, [report]);
}

function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now() / 1000), ms);
    return () => window.clearInterval(id);
  }, [ms]);
  return now;
}

export interface CockpitProps {
  onExit: () => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
  /** The app's overlays (toasts, dialogs), shown over the cockpit. */
  children?: ReactNode;
}

export function Cockpit(p: CockpitProps) {
  const t = useT();
  const { facts } = useLaserLive();
  const { status: s, unsupported } = useProduction(true);
  const job = useJobStatus(facts.link);
  const online = onlineOf(facts);
  const now = useNow(1000);
  const phase = cockpitPhase(s, online, facts.machine, now);
  const [busy, setBusy] = useState(false);
  const report = p.report;
  useCockpitLock(report);

  const act = useCallback(
    (fn: () => Promise<unknown>, done?: string) => {
      setBusy(true);
      fn()
        .then(() => done && p.notify(done))
        .catch(report)
        .finally(() => {
          setBusy(false);
          void refreshProduction();
          facts.refreshMachine();
        });
    },
    [p, report, facts],
  );

  const stop = () => {
    api
      .stop()
      .then((res) => p.notify(stopToast(res)))
      .catch(report)
      .finally(() => {
        void refreshProduction();
        facts.refreshMachine();
      });
  };

  const j = s?.job;
  const framing = facts.machine?.framing === true;
  const canPause = !!j?.active && !j.paused && j.state === "running";
  const canResume = !!j?.active && j.paused;
  const title = s ? (s.project ?? s.document) : t("prod.title");

  return (
    <div className={`cockpit tone-${phase.tone}`} role="region" aria-label={t("prod.cockpit")}>
      <header className="cockpit-bar">
        <button className="btn cockpit-exit" onClick={p.onExit} title={t("prod.exitTitle")}>
          <Icon name="arrowLeft" size={20} />
          <span>{t("prod.exit")}</span>
        </button>
        <div className="cockpit-title">
          <strong>{title}</strong>
          {s && <span className="cockpit-sub">{s.device}</span>}
        </div>
        {s?.lock.locked && (
          <span className="cockpit-lock" title={t("prod.lockedTitle")}>
            <Icon name="lock" size={16} />
            <span>{t("prod.locked")}</span>
          </span>
        )}
        <button className="btn btn-stop cockpit-stop" onClick={stop} title={t("top.stopTitle")}>
          <Icon name="stop" size={24} /> {t("top.stop")}
        </button>
      </header>

      {unsupported ? (
        <p className="cockpit-unsupported hint">{t("prod.unsupported")}</p>
      ) : (
        <main className="cockpit-grid">
          <section className={`cockpit-phase cp-${phase.kind}`} aria-live="polite">
            {phase.waitingForYou && <div className="cp-badge">{t("prod.waitingForYou")}</div>}
            <div className="cp-title">{phase.title}</div>
            {phase.seconds !== null && (
              <>
                <div className="cp-clock">{clock(phase.seconds)}</div>
                {phase.secondsLabel && <div className="cp-clock-label">{phase.secondsLabel}</div>}
              </>
            )}
            {phase.pct !== null && (
              <div className="cp-bar" aria-hidden="true">
                <span style={{ width: `${phase.pct}%` }} />
              </div>
            )}
            {phase.detail && <p className="cp-detail">{phase.detail}</p>}
            <div className="cp-actions">
              {phase.kind === "layer" && (
                <button className="btn cp-continue" disabled={busy} onClick={() => act(() => prodApi.continueRun(s?.layer_pause.run_id), t("prod.continued"))}>
                  <Icon name="skipForward" size={24} />
                  {t("prod.continue")}
                </button>
              )}
              {canPause && (
                <button className="btn cp-big" disabled={busy} onClick={() => act(() => jobApi.pause(), t("lz.paused"))}>
                  <Icon name="pause" size={22} />
                  {t("lz.pause")}
                </button>
              )}
              {canResume && (
                <button className="btn cp-big" disabled={busy} onClick={() => act(() => jobApi.resume(), t("lz.resumed"))}>
                  <Icon name="play" size={22} />
                  {t("lz.resume")}
                </button>
              )}
              {!j?.active && (
                <div className="cp-start">
                  <StartButton variant="sheet" />
                </div>
              )}
              {!j?.active &&
                (framing ? (
                  <button className="btn cp-big" onClick={() => runHotkeyAction("laser.frameStop")}>
                    <Icon name="frameStop" size={22} />
                    {t("tb.stopFrame")}
                  </button>
                ) : (
                  <button className="btn cp-big" disabled={!online || !!facts.designOnly} onClick={() => runHotkeyAction("laser.frameSquare")}>
                    <Icon name="frame" size={22} />
                    {t("tb.frame")}
                  </button>
                ))}
            </div>
            <PendingApprovals machine={facts.machine} docRevision={s?.revision ?? null} onChanged={() => facts.refreshMachine()} report={report} />
            {!noPinMode(facts.machine) && facts.machine && <p className="hint cp-pin">{t("prod.pinHint")}</p>}
          </section>

          {s && <CounterCard s={s} job={job} report={report} />}
          {s && <SlotsCard s={s} report={report} />}
          {s && <StatsCard s={s} act={act} busy={busy} />}
          {s && <LedgerCard s={s} act={act} />}
        </main>
      )}
      {p.children}
    </div>
  );
}

function CounterCard({ s, job, report }: { s: ProductionStatus; job: ReturnType<typeof useJobStatus>; report: (e: unknown) => void }) {
  const t = useT();
  const c = counterText(s.cycle);
  return (
    <section className="cockpit-card cockpit-counter">
      <h3>{t("prod.done")}</h3>
      <div className="cc-count" aria-live="polite">
        {c.text}
      </div>
      {c.share !== null && (
        <div className="cp-bar cc-bar" aria-hidden="true">
          <span style={{ width: `${Math.round(c.share * 100)}%` }} />
        </div>
      )}
      {s.cycle.left !== null && (
        <p className="cc-eta">
          {s.cycle.eta_s !== null ? t("prod.leftEta", { n: s.cycle.left, time: clock(s.cycle.eta_s) }) : t("prod.left", { n: s.cycle.left })}
        </p>
      )}
      <JobCounter state={job} report={report} />
    </section>
  );
}

function SlotsCard({ s, report }: { s: ProductionStatus; report: (e: unknown) => void }) {
  const t = useT();
  const plan = s.slots;
  const setSerial = (params: Record<string, unknown>) =>
    void execute({ id: "var.set_serial", params })
      .then(() => refreshProduction())
      .catch(report);
  const step = (steps: number) =>
    void execute({ id: "var.advance", params: { steps } })
      .then(() => refreshProduction())
      .catch(report);
  return (
    <section className="cockpit-card cockpit-next">
      <h3>{t("prod.next")}</h3>
      {plan.slots.length === 0 ? (
        <p className="hint">{t("prod.noSlots")}</p>
      ) : (
        <>
          <div className="cn-head" aria-live="polite">
            {nextValues(plan)}
          </div>
          {s.slots_refusal && (
            <p className="run-hint cn-refusal" role="alert">
              {s.slots_refusal}
            </p>
          )}
          <ul className="cn-slots">
            {plan.slots.map((slot) => {
              const l = slotLine(slot);
              return (
                <li key={slot.offset}>
                  <button
                    className={`cn-slot st-${l.state}`}
                    aria-pressed={l.state !== "empty"}
                    title={l.state === "empty" ? t("prod.slotLoadTitle") : t("prod.slotEmptyTitle")}
                    onClick={() => setSerial({ empty_slots: toggleEmpty(plan, slot.offset) })}
                  >
                    <span className="cn-num">{slot.number}</span>
                    <span className="cn-label">{l.label}</span>
                    <span className="cn-value">{l.state === "empty" ? t("prod.slotEmpty") : l.state === "past_end" ? t("prod.slotPastEnd") : l.value}</span>
                  </button>
                </li>
              );
            })}
          </ul>
          <div className="cn-tools">
            <button className="btn small" onClick={() => step(-1)} title={t("prod.prevSetTitle")}>
              {t("prod.prevSet")}
            </button>
            <button className="btn small" onClick={() => step(1)} title={t("prod.nextSetTitle")}>
              {t("prod.nextSet")}
            </button>
            <label className="cn-check">
              <input type="checkbox" checked={plan.stop_at_end} onChange={(e) => setSerial({ stop_at_end: e.target.checked })} />
              {t("prod.stopAtEnd", { end: plan.end })}
            </label>
          </div>
        </>
      )}
    </section>
  );
}

function StatsCard({ s, act, busy }: { s: ProductionStatus; act: (fn: () => Promise<unknown>, done?: string) => void; busy: boolean }) {
  const t = useT();
  const ex = s.extractor;
  const m = s.maintenance;
  return (
    <section className="cockpit-card cockpit-stats">
      <h3>{t("prod.times")}</h3>
      <dl className="cs-times">
        <dt>{t("prod.lastBurn")}</dt>
        <dd>{shortDuration(s.cycle.last_burn_s)}</dd>
        <dt>{t("prod.lastCycle")}</dt>
        <dd>{shortDuration(s.cycle.last_cycle_s)}</dd>
        <dt>{t("prod.avgCycle")}</dt>
        <dd>{shortDuration(s.cycle.average_cycle_s)}</dd>
        <dt>{t("prod.meter")}</dt>
        <dd>{hours(m.hours)}</dd>
      </dl>
      {ex.configured && (
        <div className={`cs-extractor ${ex.state.on ? "on" : "off"}`}>
          <Icon name="fan" size={20} />
          <span>{ex.state.on ? t("prod.exOn") : t("prod.exOff")}</span>
          {ex.state.off_at_unix_s !== null && <span className="hint">{t("prod.exRunOn")}</span>}
          <button className="btn small" disabled={busy} onClick={() => act(() => prodApi.switchExtractor(!ex.state.on))}>
            {ex.state.on ? t("prod.exSwitchOff") : t("prod.exSwitchOn")}
          </button>
          {ex.state.last_error && (
            <span className="run-hint" role="alert">
              {ex.state.last_error}
            </span>
          )}
        </div>
      )}
      {m.due.map((r) => (
        <div key={r.id} className="cs-due" role="status">
          <Icon name="wrench" size={18} />
          <span>{t("prod.dueLine", { name: r.name })}</span>
          <button className="btn small" disabled={busy} onClick={() => act(() => prodApi.reminderDone(r.id), t("prod.remDone"))}>
            {t("prod.remMarkDone")}
          </button>
        </div>
      ))}
    </section>
  );
}

function LedgerCard({ s, act }: { s: ProductionStatus; act: (fn: () => Promise<unknown>, done?: string) => void }) {
  const t = useT();
  const runs = [...s.ledger].reverse().slice(0, 3);
  if (runs.length === 0) return null;
  return (
    <section className="cockpit-card cockpit-ledger">
      <h3>{t("prod.ledger")}</h3>
      {runs.map((e) => (
        <div key={`${e.run_id}-${e.time_unix_s}`} className="cl-run">
          <div className="cl-head">
            <span>{new Date(e.time_unix_s * 1000).toLocaleTimeString()}</span>
            <span className={`cl-state st-${e.state}`}>{e.remark_of ? t("prod.remarkOf", { slot: e.remark_of.slot }) : e.state}</span>
          </div>
          <div className="cl-slots">
            {e.slots.map((sl) => (
              <button
                key={sl.number}
                className={`btn small cl-slot${sl.marked ? "" : " skipped"}`}
                disabled={!sl.marked}
                title={t("prod.remarkTitle", { slot: sl.number, value: sl.values.map((v) => v.shown).join(" · ") })}
                onClick={() => act(() => prodApi.remark(e.run_id, e.time_unix_s, sl.number, s.revision), t("prod.remarkRequested"))}
              >
                {sl.number}: {sl.values.map((v) => v.shown).join(" · ") || sl.value}
              </button>
            ))}
          </div>
        </div>
      ))}
      <p className="hint">{t("prod.remarkHint")}</p>
    </section>
  );
}
