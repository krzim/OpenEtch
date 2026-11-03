// Check every row (the contact sheet, short-list #10): a grid with one card per serial number or CSV row, each the
// design drawn as that run would burn it, the variable texts highlighted, and what is wrong with them: a name that
// runs off its tag, a glyph the font lacks, an empty CSV cell, text squeezed under half its height. Sized for checking
// a batch on the iPad before the first coin; on the phone the dialog is a full-screen sheet. Pages of 48 rows load as
// they are asked for. The same check is `var.contact_sheet` for agents.

import { useCallback, useEffect, useState } from "react";
import { execute } from "../api/client.ts";
import { errorText } from "../api/errorText.ts";
import { Dialog } from "../components/Dialog.tsx";
import { Icon } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { FLAG_KINDS, appendPage, parseSheet, rowKinds, rowTitle, sheetRequest, type FlagKind, type Sheet, type SheetRow } from "./contactSheet.ts";

const FLAG_KEY = {
  overflow: "sheet.flag.overflow",
  missing_glyph: "sheet.flag.missing_glyph",
  empty: "sheet.flag.empty",
  shrunk: "sheet.flag.shrunk",
  error: "sheet.flag.error",
  warning: "sheet.flag.warning",
} as const satisfies Record<FlagKind, string>;

export function ContactSheetDialog(p: { onClose: () => void; onGoto?: (value: number) => void }) {
  const t = useT();
  const [sheet, setSheet] = useState<Sheet | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [problemsOnly, setProblemsOnly] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    (from: number | null, more: Sheet | null) => {
      setBusy(true);
      execute(sheetRequest(from, problemsOnly))
        .then((r) => {
          const s = parseSheet(r.result);
          setSheet(more ? appendPage(more, s) : s);
          setError(null);
        })
        .catch((e) => setError(errorText(e)))
        .finally(() => setBusy(false));
    },
    [problemsOnly],
  );
  // Again when the filter changes (the dialog is modal: the design does not change under it, except by "Show").
  useEffect(() => load(null, null), [load]);

  const total = sheet ? Object.values(sheet.problems).reduce((a, b) => a + (b ?? 0), 0) : 0;
  return (
    <Dialog
      help="variable-text-and-barcodes#check-every-row"
      title={t("sheet.title")}
      size="xl"
      onClose={p.onClose}
      footer={
        <>
          {sheet?.next !== null && sheet?.next !== undefined && (
            <button className="btn" disabled={busy} onClick={() => sheet && load(sheet.next, sheet)}>
              {t("sheet.more")}
            </button>
          )}
          <button className="btn btn-primary" onClick={p.onClose}>
            {t("common.close")}
          </button>
        </>
      }
    >
      {error && <p className="form-error">{error}</p>}
      {!sheet && !error && <p className="muted">{t("common.loading")}</p>}
      {sheet && (
        <>
          <div className="sheet-bar" role="status">
            <span>{t("sheet.summary", { shown: sheet.rows.length, total: sheet.total, from: sheet.start, to: sheet.end })}</span>
            {total === 0 ? (
              <span className="sheet-chip ok">
                <Icon name="check" size={14} /> {t("sheet.noProblems")}
              </span>
            ) : (
              FLAG_KINDS.filter((k) => sheet.problems[k]).map((k) => (
                <span key={k} className={`sheet-chip ${k}`}>
                  {t(FLAG_KEY[k])} {sheet.problems[k]}
                </span>
              ))
            )}
            <label className="sheet-filter">
              <input type="checkbox" checked={problemsOnly} onChange={(e) => setProblemsOnly(e.target.checked)} /> {t("sheet.problemsOnly")}
            </label>
          </div>
          {sheet.rows.length === 0 && <p className="muted">{t(problemsOnly ? "sheet.noneWithProblems" : "sheet.empty")}</p>}
          <ul className="sheet-grid">
            {sheet.rows.map((row) => (
              <SheetCard key={row.value} row={row} sheet={sheet} onGoto={p.onGoto} />
            ))}
          </ul>
        </>
      )}
    </Dialog>
  );
}

function SheetCard(p: { row: SheetRow; sheet: Sheet; onGoto?: (value: number) => void }) {
  const t = useT();
  const { row, sheet } = p;
  const kinds = rowKinds(row);
  const bad = new Set(row.texts.filter((x) => x.flags.some((f) => f.kind !== "warning")).map((x) => x.id));
  const vb = sheet.viewBox;
  return (
    <li className={`sheet-card${kinds.length ? " bad" : ""}`}>
      <div className="sheet-card-head">
        <span className="sheet-value">#{row.value}</span>
        <span className="sheet-shown" title={rowTitle(row)}>
          {rowTitle(row) || t("sheet.blank")}
        </span>
      </div>
      {vb && (
        <svg className="sheet-art" viewBox={vb.join(" ")} preserveAspectRatio="xMidYMid meet" aria-hidden="true" focusable="false">
          {sheet.stock && <path className="sheet-stock" d={sheet.stock} />}
          {sheet.base.map((b, i) => (
            <path key={i} d={b.d} className={`sheet-base${b.fill ? " fill" : ""}${b.tool ? " tool" : ""}`} style={{ color: b.color }} />
          ))}
          {row.texts.map((x) => x.d && <path key={x.id} d={x.d} className={`sheet-var${bad.has(x.id) ? " bad" : ""}`} />)}
        </svg>
      )}
      {kinds.length > 0 && (
        <ul className="sheet-flags">
          {row.texts.flatMap((x) =>
            x.flags.map((f, i) => (
              <li key={`${x.id}-${i}`} className={`sheet-flag ${f.kind}`}>
                <b>{t(FLAG_KEY[f.kind] ?? "sheet.flag.warning")}</b> {f.message}
              </li>
            )),
          )}
        </ul>
      )}
      {p.onGoto && (
        <button className="btn btn-small sheet-goto" onClick={() => p.onGoto?.(row.value)} title={t("sheet.gotoTitle")}>
          {t("sheet.goto")}
        </button>
      )}
    </li>
  );
}
