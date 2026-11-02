// LightBurn's Console window for a GRBL gantry (shown only while one is active): type a G-code or `$` line and see the
// board's answer, step through the history with Up / Down, ask for a status report, and run macros (buttons) saved
// with the device in the engine's preferences (PUT /api/prefs `console_macros`). Every line, typed or from a macro,
// goes through POST /api/machine/console (UI only), whose policy refuses anything that could turn the laser on (M3,
// M4, S above 0, start-up blocks that do either, the realtime characters); a macro stops at its first refused or
// failed line. The pure parts live in api/grbl.ts.

import { useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client.ts";
import {
  appendLog,
  historyPush,
  historyStep,
  looksLaserOn,
  macroLines,
  macrosPatch,
  macrosProblem,
  parseGrblStatus,
  parseMacros,
  replyEntries,
  type ConsoleEntry,
  type Macro,
} from "../api/grbl.ts";
import { jobApi } from "../api/jobApi.ts";
import { onEngineEvent } from "../api/events.ts";
import { libsApi } from "../libs/api.ts";
import { useT } from "../workspace/usePrefs.ts";
import { useGrblStatus } from "./GrblMove.tsx";
import "./job.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

/** The engine's reason for refusing a line (its `error` text), else the message. */
function why(e: unknown): string {
  return e instanceof ApiError ? (e.detail ?? e.message) : e instanceof Error ? e.message : String(e);
}

/** The device's macros, reloaded when the device or the preferences change. */
function useMacros(): [Macro[], () => void] {
  const [macros, setMacros] = useState<Macro[]>([]);
  const [seq, setSeq] = useState(0);
  useEffect(
    () =>
      onEngineEvent(
        (ev) => (ev.type === "device_changed" || (ev.type === "machine" && ev.event === "prefs")) && setSeq((n) => n + 1),
      ),
    [],
  );
  useEffect(() => {
    let alive = true;
    libsApi
      .prefs()
      .then((v) => alive && setMacros(parseMacros(v)))
      .catch(() => alive && setMacros([]));
    return () => {
      alive = false;
    };
  }, [seq]);
  return [macros, () => setSeq((n) => n + 1)];
}

/** One macro being edited: its lines as text. */
interface Draft {
  name: string;
  text: string;
}

export function GrblConsole(p: { notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const { status, reload } = useGrblStatus(false);
  const [log, setLog] = useState<ConsoleEntry[]>([]);
  const [line, setLine] = useState("");
  const [hist, setHist] = useState<string[]>([]);
  const [hIndex, setHIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const [macros, reloadMacros] = useMacros();
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const held = !!status && (status.running || status.framing);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [log]);

  const add = (more: ConsoleEntry[]) => setLog((l) => appendLog(l, more));

  /** Send one line; true when the board answered ok. */
  const sendOne = async (text: string): Promise<boolean> => {
    add([{ kind: "sent", text }]);
    try {
      const reply = await jobApi.console(text);
      const entries = replyEntries(reply);
      add(entries);
      return entries[entries.length - 1]?.kind === "ok";
    } catch (e) {
      add([{ kind: "error", text: t("console.refused", { why: why(e) }) }]);
      return false;
    }
  };

  const send = async () => {
    const text = line.trim();
    if (!text || busy) return;
    const next = historyPush(hist, text);
    setHist(next);
    setHIndex(next.length);
    setLine("");
    setBusy(true);
    await sendOne(text);
    setBusy(false);
    reload();
  };

  const runMacro = async (m: Macro) => {
    if (busy) return;
    setBusy(true);
    add([{ kind: "info", text: t("console.macroRun", { name: m.name }) }]);
    for (let i = 0; i < m.lines.length; i++) {
      if (!(await sendOne(m.lines[i]))) {
        add([{ kind: "info", text: t("console.macroStopped", { name: m.name, n: i + 1 }) }]);
        break;
      }
    }
    setBusy(false);
    reload();
  };

  const status_ = async () => {
    reload();
    try {
      const s = parseGrblStatus(await jobApi.grblStatus(true));
      const pos = s?.machine ? `  X ${s.machine[0].toFixed(3)}  Y ${s.machine[1].toFixed(3)}` : "";
      add([{ kind: "reply", text: s?.state ? `<${s.state}>${pos}` : t("console.noStatus") }]);
    } catch (e) {
      add([{ kind: "error", text: why(e) }]);
    }
  };

  const onKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void send();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      const s = historyStep(hist, hIndex, e.key === "ArrowUp" ? -1 : 1);
      setHIndex(s.index);
      setLine(s.text);
    }
  };

  const saveDrafts = () => {
    if (!drafts) return;
    const list: Macro[] = drafts.map((d) => ({ name: d.name, lines: macroLines(d.text) }));
    const problem = macrosProblem(list);
    if (problem) return p.notify(problem, "error");
    libsApi
      .prefsEdit(macrosPatch(list))
      .then(() => {
        p.notify(t("console.macrosSaved"));
        setDrafts(null);
        reloadMacros();
      })
      .catch(p.report);
  };

  return (
    <div className="grbl-console">
      {held && <p className="hint warn-text">{t("console.busy")}</p>}
      <div className="grbl-console-log" ref={logRef} role="log" aria-live="polite">
        {log.length === 0 ? (
          <p className="hint">{t("console.empty")}</p>
        ) : (
          log.map((e, i) => (
            <div key={i} className={`grbl-log-${e.kind}`}>
              {e.kind === "sent" ? `> ${e.text}` : e.text}
            </div>
          ))
        )}
      </div>
      <div className="grbl-console-input">
        <input
          type="text"
          value={line}
          maxLength={120}
          placeholder={t("console.placeholder")}
          aria-label={t("console.placeholder")}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          disabled={held}
          onChange={(e) => setLine(e.target.value)}
          onKeyDown={onKey}
        />
        <button className="btn small btn-primary" disabled={busy || held || !line.trim()} onClick={() => void send()}>
          {t("console.send")}
        </button>
      </div>
      {line && looksLaserOn(line) && <p className="hint warn-text">{t("console.macroWarn")}</p>}
      <div className="btn-row">
        <button className="btn small" onClick={() => void status_()}>
          {t("console.status")}
        </button>
        <button className="btn small" onClick={() => setLog([])}>
          {t("console.clear")}
        </button>
      </div>

      <fieldset className="job-fieldset">
        <legend>{t("console.macros")}</legend>
        {drafts === null ? (
          <>
            <div className="grbl-macros">
              {macros.length === 0 && <span className="hint">{t("console.noMacros")}</span>}
              {macros.map((m, i) => (
                <button key={i} className="btn small" disabled={busy || held} title={m.lines.join("\n")} onClick={() => void runMacro(m)}>
                  {m.name}
                </button>
              ))}
            </div>
            <button className="btn small" onClick={() => setDrafts(macros.map((m) => ({ name: m.name, text: m.lines.join("\n") })))}>
              {t("console.editMacros")}
            </button>
          </>
        ) : (
          <>
            {drafts.map((d, i) => {
              const warn = macroLines(d.text).some(looksLaserOn);
              const set = (patch: Partial<Draft>) => setDrafts(drafts.map((x, k) => (k === i ? { ...x, ...patch } : x)));
              return (
                <div key={i} className="grbl-macro-edit">
                  <label className="field">
                    <span className="field-label">{t("console.macroName")}</span>
                    <span className="field-input">
                      <input type="text" value={d.name} maxLength={40} onChange={(e) => set({ name: e.target.value })} />
                    </span>
                  </label>
                  <label className="grbl-macro-lines">
                    <span className="field-label">{t("console.macroLines")}</span>
                    <textarea rows={3} value={d.text} spellCheck={false} onChange={(e) => set({ text: e.target.value })} />
                  </label>
                  {warn && <p className="hint warn-text">{t("console.macroWarn")}</p>}
                  <button className="btn small" onClick={() => setDrafts(drafts.filter((_, k) => k !== i))}>
                    {t("console.removeMacro")}
                  </button>
                </div>
              );
            })}
            <div className="btn-row">
              <button className="btn small" onClick={() => setDrafts([...drafts, { name: t("console.newMacro", { n: drafts.length + 1 }), text: "" }])}>
                {t("console.addMacro")}
              </button>
              <button className="btn small btn-primary" onClick={saveDrafts}>
                {t("console.saveMacros")}
              </button>
              <button className="btn small" onClick={() => setDrafts(null)}>
                {t("common.cancel")}
              </button>
            </div>
          </>
        )}
      </fieldset>
      <p className="hint">{t("console.policy")}</p>
    </div>
  );
}
