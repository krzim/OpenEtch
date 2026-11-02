// LightBurn's File List window for a Ruida gantry (shown only while one is active; unverified on hardware): the files
// stored in the controller's memory, Send (the current job stored under a name, never started), Start and Delete.
// Start fires the laser, so it only asks for a run (POST /api/machine/files/{index}/run): the run waits in the Laser
// window's Runs card and starts only when approved there (the operator PIN, or Start in the no-PIN mode), with the
// same gates as any run (Job Checklist; Require Framing refuses stored files, which cannot be framed). Every request
// here is UI only; the engine refuses MCP clients. The pure parts live in api/ruida.ts.

import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client.ts";
import { onEngineEvent } from "../api/events.ts";
import { jobApi } from "../api/jobApi.ts";
import { fileNameProblem, parseFiles, suggestFileName, type StoredFile } from "../api/ruida.ts";
import { useT } from "../workspace/usePrefs.ts";
import "./job.css";
import "./ruida.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

function why(e: unknown): string {
  return e instanceof ApiError ? (e.detail ?? e.message) : e instanceof Error ? e.message : String(e);
}

export function RuidaFiles(p: { notify: Notify; projectName: string | null; selection: readonly string[] }) {
  const t = useT();
  const [files, setFiles] = useState<StoredFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState(() => suggestFileName(p.projectName));
  const [onlySelected, setOnlySelected] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<StoredFile | null>(null);
  const alive = useRef(true);

  const reload = useCallback(() => {
    jobApi
      .ruidaFiles()
      .then((v) => {
        if (!alive.current) return;
        setFiles(parseFiles(v));
        setError(null);
      })
      .catch((e) => {
        if (!alive.current) return;
        setFiles(null);
        setError(why(e));
      });
  }, []);
  useEffect(() => {
    alive.current = true;
    reload();
    return () => {
      alive.current = false;
    };
  }, [reload]);
  useEffect(
    () =>
      onEngineEvent(
        (ev) =>
          (ev.type === "device_changed" || ev.type === "run_finished" || (ev.type === "machine" && ev.event === "controller_files_changed")) &&
          reload(),
      ),
    [reload],
  );

  const task = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      p.notify(why(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const nameProblem = fileNameProblem(name);
  const send = () =>
    task(async () => {
      const ids = onlySelected && p.selection.length > 0 ? [...p.selection] : undefined;
      await jobApi.ruidaSend(name, ids);
      p.notify(t("rd.sent", { name }));
      reload();
    });
  const start = (f: StoredFile) =>
    task(async () => {
      await jobApi.ruidaRun(f.index, f.name);
      p.notify(t("rd.startRequested", { name: f.name }));
    });
  const remove = (f: StoredFile) =>
    task(async () => {
      await jobApi.ruidaDelete(f.index, f.name);
      setConfirmDelete(null);
      p.notify(t("rd.deleted", { name: f.name }));
      reload();
    });

  return (
    <div className="ruida-files" aria-label={t("win.files")}>
      <div className="row">
        <button className="btn" disabled={busy} onClick={reload}>
          {t("rd.refresh")}
        </button>
      </div>
      {error && <p className="run-error">{error}</p>}
      {files && files.length === 0 && <p className="run-hint">{t("rd.empty")}</p>}
      {files && files.length > 0 && (
        <table className="ruida-file-table">
          <thead>
            <tr>
              <th>#</th>
              <th>{t("rd.name")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {files.map((f) => (
              <tr key={`${f.index}-${f.name}`}>
                <td>{f.index}</td>
                <td>{f.name}</td>
                <td>
                  {confirmDelete?.index === f.index ? (
                    <>
                      <button className="btn btn-warn" disabled={busy} onClick={() => void remove(f)}>
                        {t("rd.confirmDelete")}
                      </button>
                      <button className="btn" onClick={() => setConfirmDelete(null)}>
                        {t("common.cancel")}
                      </button>
                    </>
                  ) : (
                    <>
                      <button className="btn" disabled={busy} title={t("rd.startTitle")} onClick={() => void start(f)}>
                        {t("rd.start")}
                      </button>
                      <button className="btn" disabled={busy} onClick={() => setConfirmDelete(f)}>
                        {t("rd.delete")}
                      </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <fieldset className="ruida-send">
        <legend>{t("rd.send")}</legend>
        <label>
          {t("rd.name")}{" "}
          <input value={name} maxLength={32} onChange={(e) => setName(e.target.value)} aria-invalid={nameProblem !== null} />
        </label>
        {p.selection.length > 0 && (
          <label>
            <input type="checkbox" checked={onlySelected} onChange={(e) => setOnlySelected(e.target.checked)} /> {t("rd.onlySelected")}
          </label>
        )}
        {nameProblem && name.length > 0 && <p className="run-hint">{nameProblem}</p>}
        <button className="btn btn-primary" disabled={busy || nameProblem !== null} title={t("rd.sendTitle")} onClick={() => void send()}>
          {t("rd.sendButton")}
        </button>
      </fieldset>
      <p className="run-hint">{t("rd.hint")}</p>
    </div>
  );
}
