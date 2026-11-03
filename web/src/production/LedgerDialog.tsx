// The serial ledger window (engine `production::ledger`, next to the job log): which serial number or CSV row went on
// which slot in which run, the slots left out (empty, past End), and a re-mark per slot: a pending run that marks that
// slot's pocket again with the same value, approved like any run (Laser window or cockpit). CSV export.

import { useEffect, useState } from "react";
import { errorText } from "../api/errorText.ts";
import { saveDownload } from "../libs/ui.tsx";
import { WsModal } from "../workspace/WsModal.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { prodApi } from "./api.ts";
import { ledgerRows, type LedgerEntry } from "./model.ts";
import "./production.css";

export function LedgerDialog(p: { revision: number | null; notify: (m: string) => void; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const [entries, setEntries] = useState<LedgerEntry[] | null>(null);
  const [limit, setLimit] = useState(200);
  const [error, setError] = useState<string | null>(null);
  const load = () =>
    prodApi
      .ledger(limit)
      .then((r) => {
        setEntries(r.entries as LedgerEntry[]);
        setError(null);
      })
      .catch((e) => {
        setEntries([]);
        setError(errorText(e));
      });
  useEffect(() => void load(), [limit]);
  const rows = entries ? ledgerRows(entries) : [];
  const remark = (run: string, time: number, slot: number) =>
    void prodApi
      .remark(run, time, slot, p.revision)
      .then((r) => p.notify(t("prod.remarkRequestedValue", { value: r.shown.join(" · ") })))
      .catch(p.report);
  return (
    <WsModal
      help="serial-ledger#the-serial-ledger-window"
      title={t("prod.ledgerTitle")}
      size="xl"
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
          <button className="btn" onClick={() => void saveDownload(prodApi.ledgerCsv(limit), "OpenEtchSerialLedger.csv").catch(p.report)}>
            {t("jlog.csv")}
          </button>
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      {error && (
        <p className="run-hint" role="alert">
          {error}
        </p>
      )}
      {entries === null ? (
        <p className="muted">{t("common.loading")}</p>
      ) : rows.length === 0 ? (
        <p className="hint">{t("prod.ledgerEmpty")}</p>
      ) : (
        <div className="lx-table-wrap">
          <table className="lx-table prod-ledger">
            <thead>
              <tr>
                <th>{t("jlog.time")}</th>
                <th>{t("prod.lgRun")}</th>
                <th>{t("jlog.project")}</th>
                <th className="num">{t("prod.lgSlot")}</th>
                <th>{t("prod.lgPocket")}</th>
                <th>{t("prod.lgValue")}</th>
                <th>{t("jlog.state")}</th>
                <th aria-label={t("prod.remark")} />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.key} className={r.marked ? "" : "prod-skipped"}>
                  <td className="prod-nowrap">{new Date(r.time_unix_s * 1000).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}</td>
                  <td className="prod-nowrap">
                    {r.run_id}
                    {r.remarkOf && <span className="hint"> ({t("prod.lgRemarkOf", { of: r.remarkOf })})</span>}
                  </td>
                  <td>{r.project}</td>
                  <td className="num">{r.slot}</td>
                  <td>{r.label}</td>
                  <td className="prod-value prod-nowrap">{r.marked ? r.shown : "–"}</td>
                  <td>{r.marked ? r.state : r.skipped === "past_end" ? t("prod.slotPastEnd") : t("prod.slotEmpty")}</td>
                  <td>
                    {r.marked && (
                      <button className="btn small" title={t("prod.remarkTitle", { slot: r.slot, value: r.shown })} onClick={() => remark(r.run_id, r.time_unix_s, r.slot)}>
                        {t("prod.remark")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="hint">{t("prod.remarkHint")}</p>
    </WsModal>
  );
}
