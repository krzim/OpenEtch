// LightBurn's Move window for a GRBL gantry (shown only while one is active): the head's position from the board's
// status reports, jog buttons with a distance and a speed, arrow-key jogging while the window has the focus, Home,
// Unlock, Go to origin and Move to a position. Every move is a GRBL jog (`$J=`) or the homing cycle, which the engine
// sends only with the laser off (POST /api/machine/jog, /home, /unlock: UI only, refused while a run or a frame holds
// the machine). The pure parts live in api/grbl.ts.

import { useCallback, useEffect, useRef, useState } from "react";
import { onEngineEvent } from "../api/events.ts";
import { JOG_STEPS, fieldToMachine, jogDelta, jogKey, jogProblem, machineToField, parseGrblStatus, positionProblem, type GrblStatusView, type JogDir } from "../api/grbl.ts";
import { jobApi } from "../api/jobApi.ts";
import { useT } from "../workspace/usePrefs.ts";
import { NumberField } from "./ui.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

/** How often an open Move or Console window asks the board for its status, ms. */
const POLL_MS = 1000;

/**
 * GET /api/machine/grbl, refreshed every second while `poll` and the page is visible (each refresh is a `?` status
 * request to an idle board; the engine skips it while a run, frame or homing holds the machine), and when a run ends
 * or the device changes. `reload` refreshes at once.
 */
export function useGrblStatus(poll: boolean): { status: GrblStatusView | null; reload: () => void } {
  const [status, setStatus] = useState<GrblStatusView | null>(null);
  const alive = useRef(true);
  const reload = useCallback(() => {
    jobApi
      .grblStatus(true)
      .then((v) => alive.current && setStatus(parseGrblStatus(v)))
      .catch(() => alive.current && setStatus(null));
  }, []);
  useEffect(() => {
    alive.current = true;
    reload();
    return () => {
      alive.current = false;
    };
  }, [reload]);
  useEffect(() => onEngineEvent((ev) => (ev.type === "run_finished" || ev.type === "device_changed") && reload()), [reload]);
  useEffect(() => {
    if (!poll) return;
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible") reload();
    }, POLL_MS);
    return () => window.clearInterval(id);
  }, [poll, reload]);
  return { status, reload };
}

const fmt = (v: number) => v.toFixed(2);

const GRID: (JogDir | null)[] = ["nw", "n", "ne", "w", null, "e", "sw", "s", "se"];
const ARROW: Record<JogDir, string> = { n: "↑", s: "↓", e: "→", w: "←", ne: "↗", nw: "↖", se: "↘", sw: "↙" };

export function GrblMove(p: { notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const { status, reload } = useGrblStatus(true);
  const [step, setStep] = useState(10);
  const [feed, setFeed] = useState<number | null>(null);
  const [target, setTarget] = useState<[number | null, number | null]>([null, null]);
  const [busy, setBusy] = useState(false);
  const profile = status?.profile ?? null;
  const speed = feed ?? profile?.frame_feed_mm_min ?? 3000;
  const held = !!status && (status.running || status.framing);
  const startup = (status?.unsafeStartup.length ?? 0) > 0;
  const blocked = !profile || held || startup || busy;

  const act = (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    fn()
      .then(() => ok && p.notify(ok))
      .catch(p.report)
      .finally(() => {
        setBusy(false);
        reload();
      });
  };
  const jog = (dir: JogDir, s = step) => {
    if (!profile) return;
    const why = jogProblem(profile, s, speed);
    if (why) return p.notify(why, "error");
    // Jogs are not queued behind each other here: one request at a time, like a button held in LightBurn.
    act(() => jobApi.jog(jogDelta(dir, s), speed));
  };
  const goTo = (m: [number, number]) => {
    if (!profile) return;
    const why = positionProblem(profile, m) ?? jogProblem(profile, 1, speed);
    if (why) return p.notify(why, "error");
    const [x, y] = machineToField(profile, m);
    act(() => jobApi.jog({ x, y }, speed));
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLSelectElement) return;
    if (e.key === "Escape") {
      e.preventDefault();
      void jobApi.jogCancel().catch(p.report);
      return;
    }
    const k = jogKey(e, step);
    if (!k || blocked) return;
    e.preventDefault();
    e.stopPropagation();
    jog(k.dir, k.step);
  };

  const jogTitle = (d: JogDir) =>
    d === "n" ? t("move.up") : d === "s" ? t("move.down") : d === "w" ? t("move.left") : d === "e" ? t("move.right") : t("move.diag");
  const pos = status?.machine ?? (status?.field && profile ? fieldToMachine(profile, status.field) : null);
  return (
    <div className="grbl-move" tabIndex={0} onKeyDown={onKey} aria-label={t("win.move")}>
      <div className="grbl-pos">
        <span className="grbl-pos-label">{t("move.position")}</span>
        <span className="grbl-pos-value">{pos ? `X ${fmt(pos[0])}  Y ${fmt(pos[1])}` : t("move.unknown")}</span>
        <button className="btn small" title={t("move.refreshTitle")} onClick={reload}>
          {t("move.refresh")}
        </button>
      </div>
      {status?.state && <p className="hint">{t("move.state", { state: status.state })}</p>}
      {status?.alarm != null && <p className="hint warn-text">{t("move.alarm", { code: status.alarm })}</p>}
      {held && <p className="hint warn-text">{t("move.busy")}</p>}
      {startup && <p className="hint warn-text">{t("move.startup")}</p>}

      <NumberField label={t("move.distance")} unit="mm" value={step} min={0.001} max={profile ? Math.max(...profile.work_area_mm) : 1000} digits={3} onCommit={(v) => v !== null && setStep(v)} />
      <div className="grbl-steps" role="group" aria-label={t("move.distance")}>
        {JOG_STEPS.map((s) => (
          <button key={s} className={`btn small${s === step ? " btn-primary" : ""}`} onClick={() => setStep(s)}>
            {s}
          </button>
        ))}
      </div>
      <NumberField label={t("move.speed")} unit="mm/min" value={speed} min={1} max={profile?.max_feed_mm_min ?? 6000} digits={0} onCommit={(v) => v !== null && setFeed(v)} />

      <div className="grbl-jog-grid">
        {GRID.map((d, i) =>
          d ? (
            <button key={d} className="btn grbl-jog" disabled={blocked} title={jogTitle(d)} aria-label={jogTitle(d)} onClick={() => jog(d)}>
              {ARROW[d]}
            </button>
          ) : (
            <button key={`stop-${i}`} className="btn grbl-jog" title={t("move.cancel")} aria-label={t("move.cancel")} onClick={() => void jobApi.jogCancel().catch(p.report)}>
              ■
            </button>
          ),
        )}
      </div>

      <div className="btn-row">
        {profile?.homing && (
          <button className="btn small" disabled={blocked} title={t("move.homeTitle")} onClick={() => act(() => jobApi.home(), t("move.homed"))}>
            {t("move.home")}
          </button>
        )}
        <button className="btn small" disabled={!profile || held || busy} title={t("move.unlockTitle")} onClick={() => act(() => jobApi.unlock(), t("move.unlocked"))}>
          {t("move.unlock")}
        </button>
        <button className="btn small" disabled={blocked} title={t("move.originTitle")} onClick={() => goTo([0, 0])}>
          {t("move.origin")}
        </button>
      </div>

      <fieldset className="job-fieldset" disabled={blocked}>
        <legend>{t("move.goTo")}</legend>
        <NumberField label={t("move.x")} unit="mm" value={target[0]} min={0} max={profile?.work_area_mm[0]} digits={3} allowEmpty onCommit={(v) => setTarget([v, target[1]])} />
        <NumberField label={t("move.y")} unit="mm" value={target[1]} min={0} max={profile?.work_area_mm[1]} digits={3} allowEmpty onCommit={(v) => setTarget([target[0], v])} />
        <button
          className="btn small"
          title={t("move.goTitle")}
          disabled={target[0] === null || target[1] === null}
          onClick={() => target[0] !== null && target[1] !== null && goTo([target[0], target[1]])}
        >
          {t("move.go")}
        </button>
      </fieldset>
      <p className="hint">{t("move.keys")}</p>
      <p className="hint">{t("move.hint")}</p>
    </div>
  );
}
