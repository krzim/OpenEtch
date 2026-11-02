// The approval gates of the active device (ARCHITECTURE.md "Safety model"): Require Framing Before Start, the Job
// Checklist (every item ticked before "Confirm and fire"), Door Protect (approval refused while the enclosure is open)
// and the refusal texts, the "SIMULATOR TEST PIN" and "ENGINE SHUTTING DOWN" badges, Play Sound on Job Finished (the engine's
// `job_finished_sound` event; muted per browser), and the device's job settings page (Device Settings > Job).
// Preferences are per device and UI only (PUT /api/prefs); the engine checks every gate again on approval.

import { useEffect, useState } from "react";
import type { MachineStatus } from "../api/types.ts";
import { CheckField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import {
  CHIME,
  DEFAULT_CHECKLIST,
  MAX_CHECKLIST_ITEMS,
  approvalGates,
  checklistComplete,
  checklistFromText,
  checklistProblem,
  inputText,
  ioPorts,
  shutdownNotice,
  simPinActive,
  type Checklist,
} from "./model.ts";
import { editPrefs, setSoundMuted, soundMuted, useActiveDevice, useDevicePrefs } from "./store.ts";
import { errText } from "./ui.tsx";
import "./libs.css";

/** The badge shown while the engine approves runs with the simulator test PIN (`--sim-pin`, simulator only). */
export function SimPinBadge({ machine, className }: { machine: MachineStatus | null; className?: string }) {
  const t = useT();
  if (!simPinActive(machine)) return null;
  return (
    <span className={`lx-sim-badge${className ? ` ${className}` : ""}`} role="status" title={t("safe.simPinTitle")}>
      {t("safe.simPin")}
    </span>
  );
}

/** The badge shown while the engine shuts down (a restart or stop): new work is refused, a running job finishes. */
export function ShutdownBadge({ machine }: { machine: MachineStatus | null }) {
  const t = useT();
  const n = shutdownNotice(machine);
  if (!n) return null;
  const title = n.aborting ? t("safe.shutdownAbortingTitle") : n.waiting ? t("safe.shutdownWaitingTitle", { run: n.waiting }) : t("safe.shutdownTitle");
  return (
    <span className="lx-sim-badge lx-shutdown-badge" role="status" title={title}>
      {n.aborting ? t("safe.shutdownAborting") : t("safe.shutdown")}
    </span>
  );
}

/** The gates and the ticks of the open approval; the ticks reset when the checklist changes. */
export function useApprovalGates() {
  const { prefs } = useDevicePrefs();
  const dev = useActiveDevice();
  const gates = approvalGates(prefs, dev ? ioPorts(dev.profile) : null);
  const [ticks, setTicks] = useState<boolean[]>([]);
  const key = `${gates.checklist.enabled}|${gates.checklist.items.join("\n")}`;
  useEffect(() => setTicks([]), [key]);
  return {
    gates,
    ticks,
    setTicks,
    complete: checklistComplete(gates.checklist, ticks),
    reset: () => setTicks([]),
  };
}

export type ApprovalGatesState = ReturnType<typeof useApprovalGates>;

/**
 * Require Framing, Door Protect and the Job Checklist in the approval card's confirm step, or inline above Start
 * (no-PIN engines; `showDoor` false there, as the door is simply checked on every Start).
 */
export function ApprovalChecks({ state, disabled, showDoor = true }: { state: ApprovalGatesState; disabled?: boolean; showDoor?: boolean }) {
  const t = useT();
  const { gates, ticks, setTicks } = state;
  const list = gates.checklist;
  const door = showDoor ? gates.door : null;
  if (!gates.requireFraming && !door && !list.enabled) return null;
  return (
    <div className="lx-gates">
      {gates.requireFraming && <p className="lx-gate">{t("safe.requireFramingOn")}</p>}
      {door && <p className="lx-gate">{t("safe.doorOn", { input: inputText(door) })}</p>}
      {list.enabled && (
        <fieldset className="lx-checklist" disabled={disabled}>
          <legend>{t("safe.checklist")}</legend>
          {list.items.map((item, i) => (
            <label key={i} className="lx-check-item">
              <input
                type="checkbox"
                checked={ticks[i] === true}
                onChange={(e) =>
                  setTicks((cur) => {
                    const n = list.items.map((_, k) => cur[k] === true);
                    n[i] = e.target.checked;
                    return n;
                  })
                }
              />
              <span>{item}</span>
            </label>
          ))}
          {!state.complete && <p className="run-hint">{t("safe.tickAll")}</p>}
        </fieldset>
      )}
    </div>
  );
}

// ---- sound ----------------------------------------------------------------------------------------------------------

let audio: AudioContext | null = null;

/** The job-finished chime (Web Audio, no sound file). Browsers may keep it silent until the page had a click. */
export function playChime(): void {
  try {
    const Ctx = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctx) return;
    audio ??= new Ctx();
    const ctx = audio;
    void ctx.resume?.();
    const now = ctx.currentTime;
    for (const [f, at, len] of CHIME) {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.value = f;
      gain.gain.setValueAtTime(0.0001, now + at);
      gain.gain.exponentialRampToValueAtTime(0.25, now + at + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + at + len);
      osc.connect(gain).connect(ctx.destination);
      osc.start(now + at);
      osc.stop(now + at + len + 0.05);
    }
  } catch {
    // No audio output: nothing to play.
  }
}

/** Play the chime on the engine's `job_finished_sound` event unless muted in this browser. */
export function onJobFinishedSound(ev: { type: string; event?: unknown }): boolean {
  if (ev.type !== "machine" || ev.event !== "job_finished_sound") return false;
  if (!soundMuted.get()) playChime();
  return true;
}

// ---- Device Settings > Job ------------------------------------------------------------------------------------------

/** The active device's job preferences: Require Framing, Job Checklist, Play Sound, Load defaults on New. */
export function DeviceJobSettings({ active, report, notify }: { active: boolean; report: (e: unknown) => void; notify: (m: string) => void }) {
  const t = useT();
  const { prefs, supported } = useDevicePrefs();
  const muted = soundMuted.use();
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (supported === false) return <p className="hint">{t("safe.unsupported")}</p>;
  if (!active) return <p className="run-hint">{t("safe.activeOnly")}</p>;
  const items = text === null ? prefs.checklist.items : checklistFromText(text);
  const draft: Checklist = { enabled: prefs.checklist.enabled, items };
  const problem = text === null ? null : checklistProblem(draft);
  const edit = (patch: Record<string, unknown>, ok?: string) => {
    setBusy(true);
    editPrefs(patch)
      .then(() => ok && notify(ok))
      .catch((e) => report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  return (
    <div className="dev-page lx-jobprefs">
      <h4>{t("safe.framingTitle")}</h4>
      <CheckField label={t("safe.requireFraming")} checked={prefs.require_framing} onChange={(v) => edit({ require_framing: v })} />
      <p className="hint">{t("safe.requireFramingHint")}</p>

      <h4>{t("safe.checklist")}</h4>
      <CheckField
        label={t("safe.checklistEnable")}
        checked={prefs.checklist.enabled}
        onChange={(v) => {
          const its = items.length ? items : DEFAULT_CHECKLIST;
          const c = { enabled: v, items: its };
          const pr = checklistProblem(c);
          if (pr) return report(new Error(pr));
          setText(null);
          edit({ checklist: c });
        }}
      />
      <label className="field lx-textarea">
        <span className="field-label">{t("safe.checklistItems", { max: MAX_CHECKLIST_ITEMS })}</span>
        <textarea
          rows={Math.min(10, Math.max(4, items.length + 1))}
          value={text ?? prefs.checklist.items.join("\n")}
          placeholder={DEFAULT_CHECKLIST.join("\n")}
          disabled={busy}
          onChange={(e) => setText(e.target.value)}
        />
      </label>
      {problem && <p className="form-error">{problem}</p>}
      <div className="lx-row">
        <button className="btn small" disabled={busy || text === null || !!problem} onClick={() => edit({ checklist: draft }, t("safe.checklistSaved"))}>
          {t("safe.checklistSave")}
        </button>
        <button className="btn small" disabled={busy || text === null} onClick={() => setText(null)}>
          {t("dev.revert")}
        </button>
        {items.length === 0 && (
          <button className="btn small" onClick={() => setText(DEFAULT_CHECKLIST.join("\n"))}>
            {t("safe.checklistSuggest")}
          </button>
        )}
      </div>
      <p className="hint">{t("safe.checklistHint")}</p>

      <h4>{t("safe.soundTitle")}</h4>
      <CheckField label={t("safe.playSound")} checked={prefs.play_sound} onChange={(v) => edit({ play_sound: v })} />
      <CheckField label={t("safe.mute")} checked={muted} onChange={setSoundMuted} />
      <button className="btn small" onClick={playChime}>
        {t("safe.testSound")}
      </button>

      <h4>{t("safe.defaultsTitle")}</h4>
      <CheckField label={t("ldef.loadOnNew")} checked={prefs.load_defaults_on_new} onChange={(v) => edit({ load_defaults_on_new: v })} />
      <p className="hint">
        {prefs.layer_defaults.length ? t("safe.layerDefaults", { count: prefs.layer_defaults.length }) : t("safe.noLayerDefaults")}{" "}
        {prefs.has_optimization_defaults ? t("safe.optDefaults") : ""}
      </p>
    </div>
  );
}
