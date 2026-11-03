// Help menu additions (LightBurn's Help > Enable Debug Log, Report a Bug, Check for Updates): the engine's debug log
// switch, the bug report bundle (versions, machine status, job log outcomes, document counts and the last log lines,
// with tokens and PINs masked; no document content) downloaded as a file to attach to an issue, the recent log lines,
// and the opt-in update check (the engine asks GitHub's releases; nothing is downloaded or installed).

import { useEffect, useState } from "react";
import { CheckField } from "../components/ui.tsx";
import { DOC_LINKS } from "../workspace/HelpMenu.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import { parseUpdateCheck, updateText, type UpdateCheck } from "./model.ts";
import { editPrefs, useDevicePrefs } from "./store.ts";
import { errText, saveDownload } from "./ui.tsx";
import "./libs.css";

/** Report a Bug: the debug log switch, the log lines and the bundle download. */
export function BugReportDialog(p: { report: (e: unknown) => void; notify: (m: string) => void; onClose: () => void }) {
  const t = useT();
  const { prefs } = useDevicePrefs();
  const [lines, setLines] = useState<string[] | null>(null);
  const load = () =>
    libsApi
      .logs()
      .then((r) => setLines(Array.isArray((r as { lines?: unknown })?.lines) ? ((r as { lines: unknown[] }).lines.map(String) as string[]) : []))
      .catch((e) => {
        setLines([]);
        p.report(new Error(errText(e)));
      });
  useEffect(() => void load(), []);
  return (
    <WsModal
      help="troubleshooting#bug-reports-and-logs"
      title={t("dbg.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={() => void load()}>
            {t("jlog.refresh")}
          </button>
          <a className="btn" href={DOC_LINKS.issues} target="_blank" rel="noopener noreferrer">
            {t("dbg.openIssues")}
          </a>
          <button
            className="btn btn-primary"
            onClick={() =>
              void saveDownload(libsApi.bugReport(), "openetch-bug-report.json")
                .then((n) => p.notify(t("dbg.saved", { name: n })))
                .catch(p.report)
            }
          >
            {t("dbg.download")}
          </button>
        </>
      }
    >
      <p className="hint">{t("dbg.intro")}</p>
      <CheckField label={t("dbg.debugLog")} checked={prefs.debug_log} onChange={(v) => void editPrefs({ debug_log: v }).then(load).catch((e) => p.report(new Error(errText(e))))} />
      <p className="hint">{t("dbg.debugHint")}</p>
      <h4>{t("dbg.lines", { count: lines?.length ?? 0 })}</h4>
      <pre className="lx-log">{lines === null ? t("common.loading") : lines.slice(-300).join("\n") || t("dbg.noLines")}</pre>
    </WsModal>
  );
}

/** Check for Updates (opt-in). */
export function UpdatesDialog(p: { report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const { prefs } = useDevicePrefs();
  const [last, setLast] = useState<UpdateCheck | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    libsApi
      .updates()
      .then((r) => {
        const o = (r ?? {}) as { current?: unknown; last?: unknown };
        setCurrent(typeof o.current === "string" ? o.current : null);
        setLast(parseUpdateCheck(o.last));
      })
      .catch((e) => p.report(new Error(errText(e))));
  }, []);
  const check = () => {
    setBusy(true);
    libsApi
      .updatesCheck()
      .then((r) => setLast(parseUpdateCheck(r)))
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  return (
    <WsModal
      help="troubleshooting#check-for-updates"
      title={t("upd.title")}
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.close")}
          </button>
          <button className="btn btn-primary" disabled={busy || !prefs.check_for_updates} onClick={check}>
            {busy ? t("upd.checking") : t("upd.check")}
          </button>
        </>
      }
    >
      <CheckField label={t("upd.optIn")} checked={prefs.check_for_updates} onChange={(v) => void editPrefs({ check_for_updates: v }).catch((e) => p.report(new Error(errText(e))))} />
      <p className="hint">{t("upd.hint")}</p>
      {current && <p>{t("upd.current", { version: current })}</p>}
      {last && (
        <p className={last.error ? "form-error" : last.newer ? "run-hint" : ""}>
          {updateText(last)}{" "}
          {last.newer && last.url && (
            <a href={last.url} target="_blank" rel="noopener noreferrer">
              {t("upd.release")}
            </a>
          )}
        </p>
      )}
      {last?.checked_unix_s ? <p className="muted">{t("upd.checkedAt", { time: new Date(last.checked_unix_s * 1000).toLocaleString() })}</p> : null}
    </WsModal>
  );
}
