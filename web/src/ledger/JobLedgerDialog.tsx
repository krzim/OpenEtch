// The Job log window as the job ledger (short list #5): every run that ended, searchable, with a thumbnail of what
// burned, each layer's settings and time, the serial number / CSV values, an OK / scrap verdict with a note, and
// "Re-run as burned", which opens the stored document in a new tab and prepares a *pending* run: nothing starts until
// a person presses Start in the Laser window (or approves it with the PIN), with every check Start always has.
//
// Wide: the list and the selected run side by side. Phone (and narrow windows): the list, then the run full width
// with a Back button. Storage caps live in a small Storage dialog (UI only on the engine).

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { saveBlob } from "../api/jobApi.ts";
import { formatDuration } from "../api/laser.ts";
import { DITHER_MODES } from "../api/image.ts";
import { onEngineEvent } from "../api/events.ts";
import { Icon, NumberField } from "../components/ui.tsx";
import { getLaserFacts } from "../laser/laserContext.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { errText } from "../libs/ui.tsx";
import { ledgerApi, thumbnailUrl } from "./ledgerApi.ts";
import {
  DEFAULT_FILTER,
  bytesText,
  entryTitle,
  estimateError,
  hatches,
  imageDither,
  imageIntervalOf,
  intervalOf,
  layerMode,
  ledgerTotals,
  num,
  optionParts,
  parseLedger,
  parseLedgerSettings,
  rerunOutcome,
  settingParts,
  timeShares,
  type LedgerEntry,
  type LedgerFilter,
  type LedgerSettings,
  type LedgerUsage,
  type Verdict,
} from "./ledgerModel.ts";
import "./ledger.css";
import { HelpLink } from "../guide/HelpLink.tsx";

type Notify = (msg: string, kind?: "info" | "error") => void;
type T = ReturnType<typeof useT>;

function stateText(t: T, s: string): string {
  switch (s) {
    case "finished":
      return t("led.stateFinished");
    case "failed":
      return t("led.stateFailed");
    case "aborted":
      return t("led.stateAborted");
    case "cancelled":
      return t("led.stateCancelled");
    default:
      return s;
  }
}

function modeText(t: T, mode: string): string {
  switch (mode) {
    case "line":
      return t("led.modeLine");
    case "fill":
      return t("led.modeFill");
    case "fill_then_line":
      return t("led.modeFillLine");
    case "offset_fill":
      return t("led.modeOffsetFill");
    default:
      return mode;
  }
}

function byText(t: T, origin: string): string {
  if (origin === "ui") return t("led.byYou");
  if (origin === "script") return t("led.byScript");
  if (origin.startsWith("agent:")) return t("led.byAgent", { name: origin.slice(6) || "agent" });
  return origin || "?";
}

function when(unixS: number): string {
  return unixS ? new Date(unixS * 1000).toLocaleString() : "";
}

/** The run's thumbnail, loaded when it scrolls into view. */
function Thumb({ entry, size }: { entry: LedgerEntry; size: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!entry.thumbnail) return;
    const el = ref.current;
    let alive = true;
    const load = () => void thumbnailUrl(entry.id).then((u) => alive && setUrl(u));
    if (!el || typeof IntersectionObserver === "undefined") {
      load();
      return () => void (alive = false);
    }
    const io = new IntersectionObserver(
      (es) => {
        if (es.some((e) => e.isIntersecting)) {
          io.disconnect();
          load();
        }
      },
      { rootMargin: "200px" },
    );
    io.observe(el);
    return () => {
      alive = false;
      io.disconnect();
    };
  }, [entry.id, entry.thumbnail]);
  return (
    <div ref={ref} className="led-thumb" style={{ width: size, height: size }} aria-hidden="true">
      {url ? <img src={url} alt="" /> : <Icon name="image" size={Math.min(24, Math.round(size / 3))} />}
    </div>
  );
}

function VerdictChip({ v }: { v: Verdict | null }) {
  const t = useT();
  if (!v) return null;
  return (
    <span className={`led-verdict led-verdict-${v}`}>
      <Icon name={v === "ok" ? "check" : "close"} size={12} />
      {v === "ok" ? t("led.ok") : t("led.scrap")}
    </span>
  );
}

function Row({ e, selected, onPick }: { e: LedgerEntry; selected: boolean; onPick: () => void }) {
  const t = useT();
  const layers = e.layers.filter((l) => l.output);
  return (
    <li>
      <button type="button" className={`led-row${selected ? " selected" : ""} led-state-${e.state}`} aria-current={selected || undefined} onClick={onPick}>
        <Thumb entry={e} size={52} />
        <span className="led-row-main">
          <span className="led-row-title">
            <span className="led-ellipsis">{entryTitle(e)}</span>
            <VerdictChip v={e.verdict} />
          </span>
          <span className="led-row-sub">
            {when(e.timeUnixS)} · <span className="led-state">{stateText(t, e.state)}</span> · {formatDuration(e.durationS) || "0 s"}
          </span>
          {layers.length > 0 && (
            <span className="led-row-layers">
              {layers.slice(0, 4).map((l) => (
                <span key={l.id} className="led-layer-tag">
                  <span className="led-swatch" style={{ background: l.color }} />
                  {l.name}
                </span>
              ))}
              {layers.length > 4 && <span className="muted">+{layers.length - 4}</span>}
            </span>
          )}
        </span>
      </button>
    </li>
  );
}

function Detail(p: { e: LedgerEntry; busy: boolean; onMark: (v: Verdict | null, note: string) => void; onRerun: () => void; onOpen: () => void; onBack?: () => void; thumb?: number }) {
  const t = useT();
  const { e } = p;
  const [note, setNote] = useState(e.note ?? "");
  useEffect(() => setNote(e.note ?? ""), [e.id, e.note]);
  const shares = useMemo(() => timeShares(e.layers), [e.layers]);
  const err = estimateError(e);
  const opts = optionParts(e.options);
  const optText = (o: string) => {
    const [k, v] = o.split(":");
    switch (k) {
      case "repeat":
        return t("led.optRepeat", { n: v });
      case "continuous":
        return t("led.optContinuous");
      case "start_input":
        return t("led.optStartInput");
      case "selection":
        return t("led.optSelection", { n: v });
      case "from_segment":
        return t("led.optFromSegment", { n: v });
      case "queue":
        return t("led.optQueue", { n: v });
      default:
        return t("led.optTable");
    }
  };
  return (
    <div className="led-detail" aria-label={t("led.detailLabel", { name: entryTitle(e) })} role="region">
      {p.onBack && (
        <button type="button" className="btn small led-back" onClick={p.onBack}>
          <Icon name="arrowLeft" size={16} />
          {t("led.back")}
        </button>
      )}
      <div className="led-dhead">
        <Thumb entry={e} size={p.thumb ?? 128} />
        <div className="led-dhead-text">
          <h3 className="led-title">{entryTitle(e)}</h3>
          <div className="led-sub">
            {when(e.timeUnixS)} · <span className={`led-state led-state-${e.state}`}>{stateText(t, e.state)}</span> · {byText(t, e.origin)}
          </div>
          <dl className="kv led-kv">
            <dt>{t("led.device")}</dt>
            <dd>{e.device}</dd>
            {e.document && e.document !== e.project && (
              <>
                <dt>{t("led.document")}</dt>
                <dd>{e.document}</dd>
              </>
            )}
            <dt>{t("led.duration")}</dt>
            <dd>
              {formatDuration(e.durationS) || "0 s"}
              {e.completed > 1 && ` · ${t("led.iterations", { n: e.completed })}`}
            </dd>
            {e.estimatedS !== null && (
              <>
                <dt>{t("led.estimate")}</dt>
                <dd>
                  {formatDuration(e.estimatedS)}
                  {e.markTimeS !== null && ` · ${t("led.measured", { time: formatDuration(e.markTimeS), pct: err === null ? "" : `${err > 0 ? "+" : ""}${err} %` })}`}
                </dd>
              </>
            )}
            {opts.length > 0 && (
              <>
                <dt>{t("led.options")}</dt>
                <dd>{opts.map(optText).join(", ")}</dd>
              </>
            )}
          </dl>
          {e.error && <p className="form-error led-error">{e.error}</p>}
        </div>
      </div>

      <section className="led-section" aria-label={t("led.verdictLabel")}>
        <div className="led-verdict-bar" role="group" aria-label={t("led.verdictLabel")}>
          <button type="button" className={`btn small led-v-ok${e.verdict === "ok" ? " active" : ""}`} aria-pressed={e.verdict === "ok"} disabled={p.busy} onClick={() => p.onMark(e.verdict === "ok" ? null : "ok", note)}>
            <Icon name="check" size={16} />
            {t("led.markOk")}
          </button>
          <button type="button" className={`btn small led-v-scrap${e.verdict === "scrap" ? " active" : ""}`} aria-pressed={e.verdict === "scrap"} disabled={p.busy} onClick={() => p.onMark(e.verdict === "scrap" ? null : "scrap", note)}>
            <Icon name="close" size={16} />
            {t("led.markScrap")}
          </button>
          {e.notedBy && e.notedUnixS !== null && <span className="muted led-noted">{t("led.notedBy", { by: byText(t, e.notedBy), when: when(e.notedUnixS) })}</span>}
        </div>
        <label className="led-note">
          <span className="field-label">{t("led.note")}</span>
          <textarea rows={2} maxLength={2000} value={note} placeholder={t("led.notePlaceholder")} onChange={(ev) => setNote(ev.target.value)} />
        </label>
        {note.trim() !== (e.note ?? "") && (
          <div className="led-note-actions">
            <button type="button" className="btn small" disabled={p.busy} onClick={() => setNote(e.note ?? "")}>
              {t("common.cancel")}
            </button>
            <button type="button" className="btn small btn-primary" disabled={p.busy} onClick={() => p.onMark(e.verdict, note)}>
              {t("led.saveNote")}
            </button>
          </div>
        )}
      </section>

      <section className="led-section led-actions">
        <button type="button" className="btn btn-primary" disabled={p.busy || !e.rerun} onClick={p.onRerun} title={e.rerunReason ?? t("led.rerunTitle")}>
          <Icon name="rotateCcw" size={16} />
          {t("led.rerun")}
        </button>
        <button type="button" className="btn" disabled={p.busy || !e.rerun} onClick={p.onOpen} title={e.rerunReason ?? t("led.openTitle")}>
          <Icon name="file" size={16} />
          {t("led.open")}
        </button>
        <p className="hint">{e.rerun ? t("led.rerunHint") : (e.rerunReason ?? "")}</p>
      </section>

      {e.layers.length > 0 ? (
        <section className="led-section">
          <h4>{t("led.layers")} <HelpLink to="job-ledger#settings-snapshot" topic={t("led.layers")} /></h4>
          <div className="led-table-wrap">
            <table className="lx-table led-layers">
              <thead>
                <tr>
                  <th>{t("led.layer")}</th>
                  <th>{t("led.mode")}</th>
                  <th>{t("led.settings")}</th>
                  <th className="num">{t("led.time")}</th>
                </tr>
              </thead>
              <tbody>
                {e.layers.map((l, i) => {
                  const dither = l.images > 0 ? imageDither(l.settings) : null;
                  const interval = l.objects > l.images && hatches(l.settings) ? intervalOf(l.settings) : null;
                  const imageInterval = l.images > 0 ? imageIntervalOf(l.settings) : null;
                  return (
                    <tr key={l.id} className={l.output ? undefined : "led-off"}>
                      <td>
                        <span className="led-swatch" style={{ background: l.color }} />
                        {l.name}
                        {!l.output && <span className="muted"> ({t("led.noOutput")})</span>}
                        {l.preset && <div className="muted led-preset">{l.preset}</div>}
                      </td>
                      <td>
                        {modeText(t, layerMode(l.settings))}
                        {dither && <div className="muted">{t("led.image", { dither: DITHER_MODES.find((m) => m.value === dither)?.label ?? dither })}</div>}
                      </td>
                      <td>
                        {settingParts(l.settings).join(" · ")}
                        {interval !== null && <span className="muted"> · {t("led.interval", { mm: num(interval, 4) })}</span>}
                        {imageInterval !== null && <span className="muted"> · {t("led.imageInterval", { mm: num(imageInterval, 4) })}</span>}
                        {l.subLayers.map((s, k) => (
                          <div key={k} className="led-sub-layer">
                            + {typeof s.name === "string" && s.name ? `${s.name}: ` : ""}
                            {settingParts(s).join(" · ")}
                          </div>
                        ))}
                      </td>
                      <td className="num">
                        {l.estimatedS > 0 ? formatDuration(l.estimatedS) : "—"}
                        {l.estimatedS > 0 && shares[i] < 0.995 && <div className="muted">{Math.round(shares[i] * 100)} %</div>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      ) : (
        <p className="hint led-section">{t("led.noLayers")}</p>
      )}

      {e.variables && (
        <section className="led-section">
          <h4>{t("led.values")}</h4>
          <dl className="kv led-kv">
            <dt>{t("led.counter")}</dt>
            <dd>
              {e.variables.current}
              {e.variables.csvFile && ` · ${e.variables.csvFile}`}
            </dd>
            {e.variables.texts.map((x) => (
              <Fragment key={x.object}>
                <dt>
                  {x.name || x.object}
                  {x.pocket && <span className="hint"> · {`${x.pocket.jigName} P${x.pocket.number}`}</span>}
                </dt>
                <dd className="led-value">{x.value || "—"}</dd>
              </Fragment>
            ))}
          </dl>
        </section>
      )}
      {e.jigs.length > 0 && (
        <section className="led-section">
          <h4>{t("led.jigs")}</h4>
          <ul className="led-jigs">
            {e.jigs.map((j) => (
              <li key={j.id}>{t("led.jigPockets", { jig: j.name || j.id, on: j.pocketsOn.join(", ") || "–", of: j.pockets })}</li>
            ))}
          </ul>
        </section>
      )}
      {e.summary && <p className="hint led-section">{e.summary}</p>}
    </div>
  );
}

/** Storage caps, in place of the list (no second sheet over the first on a phone). */
function StoragePane(p: { report: (e: unknown) => void; notify: Notify; onClose: () => void; onDirty: (d: boolean) => void }) {
  const t = useT();
  const [s, setS] = useState<LedgerSettings | null>(null);
  const [usage, setUsage] = useState<LedgerUsage | null>(null);
  const [saved, setSaved] = useState<LedgerSettings | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    ledgerApi
      .settings()
      .then((r) => {
        const v = parseLedgerSettings(r);
        setS(v.settings);
        setSaved(v.settings);
        setUsage(v.usage);
      })
      .catch((e) => p.report(new Error(errText(e))));
  }, []);
  const dirty = !!s && !!saved && JSON.stringify(s) !== JSON.stringify(saved);
  useEffect(() => p.onDirty(dirty), [dirty]);
  const save = () => {
    if (!s) return;
    setBusy(true);
    ledgerApi
      .saveSettings(s)
      .then((r) => {
        const v = parseLedgerSettings(r);
        setSaved(v.settings);
        setUsage(v.usage);
        p.notify(t("led.storageSaved"));
        p.onDirty(false);
        p.onClose();
      })
      .catch((e) => p.report(new Error(errText(e))))
      .finally(() => setBusy(false));
  };
  const set = (k: keyof LedgerSettings) => (v: number | null) => v !== null && setS((cur) => (cur ? { ...cur, [k]: Math.round(v) } : cur));
  return (
    <div className="led-storage" role="region" aria-label={t("led.storageTitle")}>
      <button type="button" className="btn small led-back" onClick={p.onClose}>
        <Icon name="arrowLeft" size={16} />
        {t("led.back")}
      </button>
      <h3 className="led-title">{t("led.storageTitle")} <HelpLink to="job-ledger#storage" topic={t("led.storageTitle")} /></h3>
      {!s ? (
        <p className="muted">{t("common.loading")}</p>
      ) : (
        <>
          <p className="hint">{t("led.storageIntro")}</p>
          <NumberField label={t("led.keepDocuments")} unit={t("led.jobs")} value={s.keepDocuments} min={0} max={2000} digits={0} onCommit={set("keepDocuments")} />
          <NumberField label={t("led.maxDocumentMb")} unit="MB" value={s.maxDocumentMb} min={1} max={200} digits={0} onCommit={set("maxDocumentMb")} />
          <p className="hint">{t("led.keepDocumentsHint")}</p>
          <NumberField label={t("led.keepThumbnails")} unit={t("led.jobs")} value={s.keepThumbnails} min={0} max={50000} digits={0} onCommit={set("keepThumbnails")} />
          <NumberField label={t("led.thumbnailPx")} unit="px" value={s.thumbnailPx} min={64} max={320} digits={0} onCommit={set("thumbnailPx")} />
          <p className="hint">{t("led.keepThumbnailsHint")}</p>
          {usage && (
            <p className="led-usage" role="status">
              {t("led.usage", {
                documents: usage.documents,
                documentSize: bytesText(usage.documentBytes),
                thumbnails: usage.thumbnails,
                thumbnailSize: bytesText(usage.thumbnailBytes),
              })}
            </p>
          )}
          {s.keepDocuments < (saved?.keepDocuments ?? 0) || s.keepThumbnails < (saved?.keepThumbnails ?? 0) ? <p className="warn-text">{t("led.pruneWarning")}</p> : null}
          <div className="led-note-actions">
            <button type="button" className="btn" onClick={p.onClose}>
              {t("common.cancel")}
            </button>
            <button type="button" className="btn btn-primary" disabled={busy || !dirty} onClick={save}>
              {t("led.save")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

export function JobLedgerDialog(p: { notify: Notify; report: (e: unknown) => void; onClose: () => void; onRequested: () => void }) {
  const t = useT();
  const lm = useLayoutMode();
  const phone = lm.layout === "phone";
  // On touch the search field is not focused on open: that would bring the on-screen keyboard over the list.
  const touch = lm.pointer === "coarse";
  const [filter, setFilter] = useState<LedgerFilter>(DEFAULT_FILTER);
  const [query, setQuery] = useState("");
  const [rows, setRows] = useState<LedgerEntry[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [showDetail, setShowDetail] = useState(false);
  const [storage, setStorage] = useState(false);
  const [storageDirty, setStorageDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const F = useRef(filter);
  F.current = filter;

  const load = () =>
    ledgerApi
      .list(F.current)
      .then((r) => setRows(parseLedger(r)))
      .catch((e) => {
        setRows([]);
        p.report(new Error(errText(e)));
      });
  useEffect(() => void load(), [filter]);
  // The search box is sent a moment after typing stops.
  useEffect(() => {
    const id = window.setTimeout(() => setFilter((f) => (f.q === query ? f : { ...f, q: query })), 250);
    return () => window.clearTimeout(id);
  }, [query]);
  // A run ended or a verdict changed elsewhere (another browser, an agent): show it.
  useEffect(
    () =>
      onEngineEvent((ev) => {
        if (ev.type === "machine" && (ev as { event?: unknown }).event === "job_log_changed") void load();
      }),
    [],
  );

  const sel = rows?.find((r) => r.id === picked) ?? (phone ? null : (rows?.[0] ?? null));
  const totals = rows ? ledgerTotals(rows) : null;

  const mark = (e: LedgerEntry, v: Verdict | null, note: string) => {
    setBusy(true);
    ledgerApi
      .mark(e.id, v, note)
      .then(() => load())
      .catch((x) => p.report(new Error(errText(x))))
      .finally(() => setBusy(false));
  };
  const rerun = (e: LedgerEntry) => {
    setBusy(true);
    ledgerApi
      .rerun(e.id)
      .then((r) => {
        const o = rerunOutcome(r);
        p.onRequested();
        p.onClose();
        if (o.runId) {
          p.notify(t("led.rerunReady", { name: entryTitle(e) }));
          getLaserFacts().openLaser();
        } else {
          p.notify(t("led.rerunNoRun", { why: o.problem ?? "" }), "error");
        }
      })
      .catch((x) => p.report(new Error(errText(x))))
      .finally(() => setBusy(false));
  };
  const open = (e: LedgerEntry) => {
    setBusy(true);
    ledgerApi
      .open(e.id)
      .then((r) => {
        const o = rerunOutcome(r);
        p.onClose();
        p.notify(o.problem ?? t("led.opened", { name: entryTitle(e) }), o.problem ? "error" : "info");
      })
      .catch((x) => p.report(new Error(errText(x))))
      .finally(() => setBusy(false));
  };
  const csv = () => {
    ledgerApi
      .csv(filter)
      .then((b) => saveBlob(b, "OpenEtchJobs.csv"))
      .catch((x) => p.report(new Error(errText(x))));
  };

  const single = phone;
  const detailShown = single && showDetail && sel;
  return (
      <WsModal
        help="job-ledger#job-log-window"
        title={t("jlog.title")}
        size="xl"
        wide
        dirty={storage && storageDirty}
        onClose={p.onClose}
        footer={
          storage ? undefined : (
          <>
            <label className="lx-inline led-limit">
              {t("jlog.show")}
              <select value={filter.limit} onChange={(e) => setFilter({ ...filter, limit: Number(e.target.value) })}>
                {[50, 200, 1000].map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </label>
            <button className="btn" onClick={() => setStorage(true)} title={t("led.storageTitle")}>
              {t("led.storage")}
            </button>
            <button className="btn" onClick={csv}>
              {t("jlog.csv")}
            </button>
            <button className="btn btn-primary" autoFocus={touch} onClick={p.onClose}>
              {t("common.close")}
            </button>
          </>
          )
        }
      >
        {storage ? (
          <StoragePane
            report={p.report}
            notify={p.notify}
            onDirty={setStorageDirty}
            onClose={() => {
              setStorage(false);
              setStorageDirty(false);
            }}
          />
        ) : (
        <div className={`led-body${single ? " single" : ""}`}>
          {!detailShown && (
            <div className="led-list-pane">
              <div className="led-filters" role="search">
                <span className="led-search">
                  <Icon name="search" size={16} />
                  <input type="search" className="led-search-input" value={query} placeholder={t("led.searchPlaceholder")} aria-label={t("led.search")} enterKeyHint="search" onChange={(e) => setQuery(e.target.value)} />
                </span>
                <select aria-label={t("led.verdictFilter")} value={filter.verdict} onChange={(e) => setFilter({ ...filter, verdict: e.target.value as LedgerFilter["verdict"] })}>
                  <option value="any">{t("led.anyVerdict")}</option>
                  <option value="ok">{t("led.ok")}</option>
                  <option value="scrap">{t("led.scrap")}</option>
                  <option value="unmarked">{t("led.unmarked")}</option>
                </select>
                <select aria-label={t("led.stateFilter")} value={filter.state} onChange={(e) => setFilter({ ...filter, state: e.target.value as LedgerFilter["state"] })}>
                  <option value="any">{t("led.anyState")}</option>
                  <option value="finished">{t("led.stateFinished")}</option>
                  <option value="failed">{t("led.stateFailed")}</option>
                  <option value="aborted">{t("led.stateAborted")}</option>
                  <option value="cancelled">{t("led.stateCancelled")}</option>
                </select>
              </div>
              {totals && rows && rows.length > 0 && (
                <p className="hint led-totals">
                  {t("led.totals", { runs: totals.runs, finished: totals.finished, failed: totals.failed, ok: totals.ok, scrap: totals.scrap, time: formatDuration(totals.seconds) || "0 s" })}
                </p>
              )}
              {rows === null ? (
                <p className="muted">{t("common.loading")}</p>
              ) : rows.length === 0 ? (
                <p className="hint">{filter.q || filter.verdict !== "any" || filter.state !== "any" ? t("led.noMatch") : t("jlog.empty")}</p>
              ) : (
                <ul className="led-list" aria-label={t("led.runs")}>
                  {rows.map((r) => (
                    <Row
                      key={r.id}
                      e={r}
                      selected={!single && sel?.id === r.id}
                      onPick={() => {
                        setPicked(r.id);
                        setShowDetail(true);
                      }}
                    />
                  ))}
                </ul>
              )}
            </div>
          )}
          {sel && (!single || detailShown) && (
            <Detail key={sel.id} e={sel} busy={busy} onMark={(v, n) => mark(sel, v, n)} onRerun={() => rerun(sel)} onOpen={() => open(sel)} onBack={single ? () => setShowDetail(false) : undefined} thumb={single ? 88 : 128} />
          )}
        </div>
        )}
      </WsModal>
  );
}
