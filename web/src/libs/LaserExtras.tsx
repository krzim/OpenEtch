// Laser window extras: Red Dot Always On (the device's IO setting, from the Laser window as LightBurn's red-light
// toggle), Batch (run several saved projects with one approval), Repeat Marking with the external axis table, the job
// log viewer (Save Job Log as CSV) and LightBurn's Print and Cut wizard (pick two registration targets, find each with
// the red dot, align the design). Every run request is a pending run that still needs the operator PIN.

import { useEffect, useMemo, useState } from "react";
import { runRequestBody } from "../api/job.ts";
import type { Invocation } from "../api/client.ts";
import { formatDuration } from "../api/laser.ts";
import { projectList } from "../api/project.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import { getRunOptions } from "../components/jobStore.ts";
import { CheckField, Icon, NumberField, PairField } from "../components/ui.tsx";
import { boxCenter, objectBounds, unionBoxes } from "../geometry.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { lengthText } from "../workspace/units.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { libsApi } from "./api.ts";
import {
  MAX_QUEUE,
  alignToTargets,
  batchBody,
  ioPorts,
  jobLogTotals,
  originText,
  parseJobLog,
  printCutProblem,
  tableBody,
  tableRole,
  type JobLogEntry,
} from "./model.ts";
import { openLibsWindow, refreshActiveDevice, useActiveDevice } from "./store.ts";
import { DropMenu, errText, saveDownload, type DropItem } from "./ui.tsx";
import "./libs.css";

type Notify = (msg: string, kind?: "info" | "error") => void;
type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

/** Red Dot Always On and the Laser tools buttons, under the Laser window's run options. */
export function LaserExtras(p: { designOnly: string | null; tools?: DropItem[]; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const dev = useActiveDevice();
  const io = dev ? ioPorts(dev.profile) : null;
  const [busy, setBusy] = useState(false);
  const setRedDot = (on: boolean) => {
    if (!dev) return;
    setBusy(true);
    libsApi
      .deviceEdit(dev.id, { io: { red_dot_always_on: on } })
      .then(() => {
        p.notify(on ? t("lx.redDotOn") : t("lx.redDotOff"));
        return refreshActiveDevice();
      })
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  return (
    <div className="lx-laser-extras">
      {io && (
        <label className="field field-check" title={t("lx.redDotTitle")}>
          <span className="field-label">{t("lx.redDot")}</span>
          <input type="checkbox" checked={io.red_dot_always_on} disabled={busy || !!p.designOnly} onChange={(e) => setRedDot(e.target.checked)} />
        </label>
      )}
      <div className="laser-buttons laser-more">
        {p.tools && <DropMenu label={t("lx.laserTools")} title={t("lx.laserToolsTitle")} items={p.tools} />}
        <button className="btn small" disabled={!!p.designOnly} onClick={() => openLibsWindow("batch")} title={t("batch.title")}>
          {t("batch.button")}
        </button>
        <button className="btn small" disabled={!!p.designOnly} onClick={() => openLibsWindow("repeatTable")} title={t("tbl.title")}>
          {t("tbl.button")}
        </button>
        <button className="btn small" onClick={() => openLibsWindow("printCut")} title={t("pc.title")}>
          {t("pc.button")}
        </button>
        <button className="btn small" onClick={() => openLibsWindow("jobLog")} title={t("jlog.title")}>
          {t("jlog.button")}
        </button>
      </div>
    </div>
  );
}

// ---- Batch ----------------------------------------------------------------------------------------------------------

export function BatchDialog(p: { notify: Notify; report: (e: unknown) => void; onClose: () => void; onRequested: () => void }) {
  const t = useT();
  const [projects, setProjects] = useState<string[] | null>(null);
  const [queue, setQueue] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    libsApi
      .projects()
      .then((r) => setProjects(projectList(r).map((x) => x.name)))
      .catch((e) => {
        setProjects([]);
        p.report(new Error(errText(e)));
      });
  }, []);
  const move = (i: number, d: -1 | 1) =>
    setQueue((q) => {
      const j = i + d;
      if (j < 0 || j >= q.length) return q;
      const n = [...q];
      [n[i], n[j]] = [n[j], n[i]];
      return n;
    });
  const request = () => {
    setBusy(true);
    Promise.resolve()
      .then(() => libsApi.requestRun(batchBody(runRequestBody({ ...getRunOptions(), cutSelected: false }), queue)))
      .then(() => {
        p.notify(t("batch.requested", { count: queue.length }));
        p.onRequested();
        p.onClose();
      })
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  return (
    <WsModal
      title={t("batch.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || queue.length === 0} onClick={request}>
            {t("batch.request", { count: queue.length })}
          </button>
        </>
      }
    >
      <p className="hint">{t("batch.intro", { max: MAX_QUEUE })}</p>
      <div className="lx-two">
        <div>
          <h4>{t("batch.saved")}</h4>
          {projects === null ? (
            <p className="muted">{t("common.loading")}</p>
          ) : projects.length === 0 ? (
            <p className="hint">{t("batch.none")}</p>
          ) : (
            <ul className="lx-list">
              {projects.map((n) => (
                <li key={n}>
                  <span className="lx-list-name">{n}</span>
                  <button className="btn small" disabled={queue.length >= MAX_QUEUE} onClick={() => setQueue((q) => [...q, n])} title={t("batch.addTitle")}>
                    <Icon name="plus" size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div>
          <h4>{t("batch.queue")}</h4>
          {queue.length === 0 ? (
            <p className="hint">{t("batch.empty")}</p>
          ) : (
            <ol className="lx-list">
              {queue.map((n, i) => (
                <li key={`${n}#${i}`}>
                  <span className="lx-list-name">{n}</span>
                  <button className="icon-btn small" aria-label={t("layers.moveUp")} disabled={i === 0} onClick={() => move(i, -1)}>
                    <Icon name="chevronUp" size={14} />
                  </button>
                  <button className="icon-btn small" aria-label={t("layers.moveDown")} disabled={i === queue.length - 1} onClick={() => move(i, 1)}>
                    <Icon name="chevronDown" size={14} />
                  </button>
                  <button className="icon-btn small" aria-label={t("batch.remove")} onClick={() => setQueue((q) => q.filter((_, k) => k !== i))}>
                    <Icon name="close" size={14} />
                  </button>
                </li>
              ))}
            </ol>
          )}
        </div>
      </div>
      <p className="run-hint">{t("batch.framingNote")}</p>
    </WsModal>
  );
}

// ---- Repeat Marking with the external axis table --------------------------------------------------------------------

export function RepeatTableDialog(p: { selection: readonly string[]; notify: Notify; report: (e: unknown) => void; onClose: () => void; onRequested: () => void }) {
  const t = useT();
  const dev = useActiveDevice();
  const role = tableRole(dev?.profile);
  const [repeat, setRepeat] = useState(Math.max(2, getRunOptions().repeat));
  const [increment, setIncrement] = useState(10);
  const [settle, setSettle] = useState(500);
  const [back, setBack] = useState(true);
  const [busy, setBusy] = useState(false);
  const unit = role === "rotary_table" ? "°" : "mm";
  const request = () => {
    setBusy(true);
    Promise.resolve()
      .then(() => libsApi.requestRun(tableBody(runRequestBody({ ...getRunOptions(), continuous: false }, { selection: p.selection }), { increment, settle_ms: settle, return_to_start: back, repeat })))
      .then(() => {
        p.notify(t("tbl.requested", { count: repeat }));
        p.onRequested();
        p.onClose();
      })
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  return (
    <WsModal
      title={t("tbl.title")}
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !role} onClick={request}>
            {t("tbl.request")}
          </button>
        </>
      }
    >
      {!role ? (
        <p className="run-hint">{t("tbl.noTable")}</p>
      ) : (
        <>
          <p className="hint">{role === "rotary_table" ? t("tbl.rotary") : t("tbl.linear")}</p>
          <NumberField label={t("tbl.repeat")} value={repeat} min={2} max={10000} digits={0} onCommit={(v) => v !== null && setRepeat(Math.round(v))} />
          <NumberField label={t("tbl.increment")} unit={unit === "mm" ? "mm" : "°"} value={increment} digits={3} onCommit={(v) => v !== null && setIncrement(v)} />
          <NumberField label={t("tbl.settle")} unit="ms" value={settle} min={0} max={60000} digits={0} onCommit={(v) => v !== null && setSettle(v)} />
          <CheckField label={t("tbl.returnToStart")} checked={back} onChange={setBack} />
          <p className="hint">{t("tbl.hint", { total: Number(((repeat - 1) * increment).toFixed(3)), unit })}</p>
        </>
      )}
    </WsModal>
  );
}

// ---- job log --------------------------------------------------------------------------------------------------------

export function JobLogDialog(p: { report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const [rows, setRows] = useState<JobLogEntry[] | null>(null);
  const [limit, setLimit] = useState(200);
  const load = () =>
    libsApi
      .jobLog(limit)
      .then((r) => setRows(parseJobLog(r)))
      .catch((e) => {
        setRows([]);
        p.report(new Error(errText(e)));
      });
  useEffect(() => void load(), [limit]);
  const totals = rows ? jobLogTotals(rows) : null;
  return (
    <WsModal
      title={t("jlog.title")}
      wide
      onClose={p.onClose}
      footer={
        <>
          <label className="lx-inline">
            {t("jlog.show")}
            <select value={limit} onChange={(e) => setLimit(Number(e.target.value))}>
              {[50, 200, 1000].map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </label>
          <button className="btn" onClick={() => void load()}>
            {t("jlog.refresh")}
          </button>
          <button className="btn" onClick={() => void saveDownload(libsApi.jobLogCsv(), "OpenEtchJobs.csv").catch(p.report)}>
            {t("jlog.csv")}
          </button>
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      {rows === null ? (
        <p className="muted">{t("common.loading")}</p>
      ) : rows.length === 0 ? (
        <p className="hint">{t("jlog.empty")}</p>
      ) : (
        <>
          {totals && <p className="hint">{t("jlog.totals", { runs: totals.runs, finished: totals.finished, failed: totals.failed, time: formatDuration(totals.seconds) || "0 s" })}</p>}
          <div className="lx-table-wrap">
            <table className="lx-table">
              <thead>
                <tr>
                  <th>{t("jlog.time")}</th>
                  <th>{t("jlog.state")}</th>
                  <th>{t("jlog.by")}</th>
                  <th>{t("jlog.project")}</th>
                  <th>{t("jlog.device")}</th>
                  <th className="num">{t("jlog.duration")}</th>
                  <th className="num">{t("jlog.runs")}</th>
                  <th>{t("jlog.summary")}</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r, i) => (
                  <tr key={`${r.run_id}#${i}`} className={`lx-state-${r.state}`}>
                    <td>{r.time_unix_s ? new Date(r.time_unix_s * 1000).toLocaleString() : ""}</td>
                    <td>{r.state}</td>
                    <td>{originText(r.origin)}</td>
                    <td>{r.project ?? "—"}</td>
                    <td>{r.device}</td>
                    <td className="num">{formatDuration(r.duration_s)}</td>
                    <td className="num">{r.completed}</td>
                    <td className="lx-sum" title={r.error ?? r.summary}>
                      {r.error ? <span className="form-error">{r.error}</span> : r.summary}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </WsModal>
  );
}

// ---- Print and Cut --------------------------------------------------------------------------------------------------

function centreOf(doc: GDocument | null, id: string | null): Vec2 | null {
  const o = id && doc ? doc.objects.find((x) => x.id === id) : undefined;
  if (!o) return null;
  const b = unionBoxes([objectBounds(o)]);
  return b ? boxCenter(b) : null;
}

/**
 * LightBurn's Print and Cut for a galvo: select a target shape and set it as Target 1, the same for Target 2, move the
 * red dot onto each printed target and set its position, then Align (rotate, move and optionally scale the design so
 * the targets land on the positions found; one undo step).
 */
export function PrintCutDialog(p: { doc: GDocument | null; selection: readonly string[]; run: Run; notify: Notify; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const { units } = usePrefs();
  const [targets, setTargets] = useState<(string | null)[]>([null, null]);
  const [dot, setDot] = useState<Vec2[]>([
    [0, 0],
    [0, 0],
  ]);
  const [positions, setPositions] = useState<(Vec2 | null)[]>([null, null]);
  const [scale, setScale] = useState(false);
  const [step, setStep] = useState(1);
  const [busy, setBusy] = useState(false);
  const centres = useMemo(() => targets.map((id) => centreOf(p.doc, id)), [targets, p.doc]);
  const name = (id: string | null) => (id ? (p.doc?.objects.find((o) => o.id === id)?.name ?? id) : t("pc.none"));
  const one = p.selection.length === 1 ? p.selection[0] : null;
  const pick = (i: number) => {
    if (!one) return;
    setTargets((x) => x.map((v, k) => (k === i ? one : v)));
    const c = centreOf(p.doc, one);
    if (c) setDot((d) => d.map((v, k) => (k === i ? c : v)));
    setPositions((x) => x.map((v, k) => (k === i ? null : v)));
  };
  const point = (i: number, at: Vec2) => {
    setDot((d) => d.map((v, k) => (k === i ? at : v)));
    libsApi.pointer(at[0], at[1]).catch((e) => p.report(new Error(errText(e))));
  };
  const nudge = (i: number, dx: number, dy: number) => point(i, [Number((dot[i][0] + dx).toFixed(4)), Number((dot[i][1] + dy).toFixed(4))]);
  const problem = printCutProblem(targets, positions, centres);
  const align = () => {
    if (problem || !targets[0] || !targets[1] || !positions[0] || !positions[1]) return;
    setBusy(true);
    p.run(alignToTargets([targets[0], targets[1]], [positions[0], positions[1]], scale))
      .then(() => {
        p.notify(t("pc.aligned"));
        void libsApi.frameStop().catch(() => undefined);
        p.onClose();
      })
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  const close = () => {
    void libsApi.frameStop().catch(() => undefined);
    p.onClose();
  };
  return (
    <WsModal
      title={t("pc.title")}
      wide
      onClose={close}
      footer={
        <>
          <button className="btn" onClick={close}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || !!problem} title={problem ?? undefined} onClick={align}>
            {t("pc.align")}
          </button>
        </>
      }
    >
      <p className="hint">{t("pc.intro")}</p>
      <div className="lx-two">
        {[0, 1].map((i) => (
          <fieldset key={i} className="job-fieldset">
            <legend>{t("pc.target", { n: i + 1 })}</legend>
            <dl className="kv">
              <dt>{t("pc.shape")}</dt>
              <dd>{name(targets[i])}</dd>
              <dt>{t("pc.inDesign")}</dt>
              <dd>{centres[i] ? `${lengthText(centres[i]![0], units, 2)}, ${lengthText(centres[i]![1], units, 2)}` : "—"}</dd>
              <dt>{t("pc.found")}</dt>
              <dd>{positions[i] ? `${lengthText(positions[i]![0], units, 2)}, ${lengthText(positions[i]![1], units, 2)}` : t("pc.notSet")}</dd>
            </dl>
            <button className="btn small" disabled={!one} title={one ? undefined : t("pc.selectOne")} onClick={() => pick(i)}>
              {t("pc.useSelected")}
            </button>
            <PairField labels={[t("ltest.centreX"), t("ltest.centreY")]} unit="mm" value={dot[i] as [number, number]} onCommit={(v) => point(i, v)} />
            <div className="lx-jog" aria-label={t("pc.jog")}>
              <button className="icon-btn small" aria-label="Y+" onClick={() => nudge(i, 0, step)}>
                <Icon name="chevronUp" size={16} />
              </button>
              <div>
                <button className="icon-btn small" aria-label="X-" onClick={() => nudge(i, -step, 0)}>
                  <Icon name="prev" size={16} />
                </button>
                <button className="btn small" disabled={!targets[i]} onClick={() => point(i, dot[i])} title={t("pc.showDotTitle")}>
                  {t("pc.showDot")}
                </button>
                <button className="icon-btn small" aria-label="X+" onClick={() => nudge(i, step, 0)}>
                  <Icon name="next" size={16} />
                </button>
              </div>
              <button className="icon-btn small" aria-label="Y-" onClick={() => nudge(i, 0, -step)}>
                <Icon name="chevronDown" size={16} />
              </button>
            </div>
            <button className="btn small btn-primary" disabled={!targets[i]} onClick={() => setPositions((x) => x.map((v, k) => (k === i ? dot[i] : v)))}>
              {t("pc.setPosition")}
            </button>
          </fieldset>
        ))}
      </div>
      <NumberField label={t("pc.step")} unit="mm" value={step} min={0.01} max={20} digits={2} onCommit={(v) => v !== null && setStep(v)} />
      <CheckField label={t("pc.scale")} checked={scale} onChange={setScale} />
      {problem && <p className="run-hint">{problem}</p>}
      <p className="hint">{t("pc.note")}</p>
    </WsModal>
  );
}
