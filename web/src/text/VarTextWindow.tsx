// LightBurn's Variable Text window, docked in the Properties panel: the document's serial number / CSV row counter
// (Start, End, Current, Advance By, Auto-Advance), Previous / Next / Reset, Test (what every variable text and barcode
// would show for a row, `var.preview_row`, changing nothing), and the CSV file for merge mode (Load / Clear). Every
// change is one engine command (`var.set_serial`, `var.advance`, `var.load_csv`, `var.clear_csv`).

import { useRef, useState } from "react";
import { execute, type Invocation } from "../api/client.ts";
import type { GDocument } from "../api/types.ts";
import { CheckField, Icon, NumberField } from "../components/ui.tsx";
import { Dock } from "../workspace/Dock.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { MAX_CSV_BYTES, TXT, advance, clearCsv, loadCsv, parseRowPreview, previewRow, serialProblem, setSerial, variablesOf, type RowPreview, type SerialFields } from "./textModel.ts";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export function VarTextWindow(p: {
  doc: GDocument;
  has: (cmd: string) => boolean;
  run: Run;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  onClose?: () => void;
}) {
  const t = useT();
  const v = variablesOf(p.doc);
  const fileRef = useRef<HTMLInputElement>(null);
  const [test, setTest] = useState<RowPreview | null>(null);
  const [testValue, setTestValue] = useState<number | null>(null);
  const can = (id: string) => p.has(id);
  const set = (f: SerialFields) => {
    const problem = serialProblem(v, f);
    if (problem) return p.notify(problem, "error");
    void p.run(setSerial(f)).catch(p.report);
  };
  const runTest = (value?: number) =>
    execute(previewRow(value))
      .then((r) => setTest(parseRowPreview(r.result)))
      .catch(p.report);
  const name = (id: string) => p.doc.objects.find((o) => o.id === id)?.name ?? id;
  const onFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (!f) return;
    if (f.size > MAX_CSV_BYTES) return p.notify(t("var.csvTooBig"), "error");
    try {
      const r = await p.run(loadCsv(await f.text(), f.name));
      const rows = (r as { variables?: { csv?: { rows?: number } } })?.variables?.csv?.rows;
      p.notify(t("var.csvLoaded", { name: f.name, rows: rows ?? "?" }));
    } catch (err) {
      p.report(err);
    }
  };
  const csv = v.csv;
  const row = csv && v.current >= 0 && v.current < csv.rows.length ? csv.rows[v.current] : null;
  return (
    <Dock
      help="variable-text-and-barcodes#the-variable-text-window"
      id="varText"
      title={t("var.title")}
      actions={
        p.onClose ? (
          <button className="icon-btn small" title={t("common.close")} aria-label={t("common.close")} onClick={p.onClose}>
            <Icon name="close" size={14} />
          </button>
        ) : undefined
      }
    >
      <div className="tx-var">
        {!can(TXT.setSerial) ? (
          <p className="hint">{t("var.unsupported")}</p>
        ) : (
          <>
            <div className="field-grid">
              <NumberField label={t("var.start")} value={v.start} step={1} digits={0} onCommit={(n) => n !== null && set({ start: Math.round(n) })} />
              <NumberField label={t("var.end")} value={v.end} step={1} digits={0} onCommit={(n) => n !== null && set({ end: Math.round(n) })} />
              <NumberField label={t("var.current")} value={v.current} step={1} digits={0} onCommit={(n) => n !== null && set({ current: Math.round(n) })} />
              <NumberField label={t("var.advanceBy")} value={v.advance_by} step={1} digits={0} onCommit={(n) => n !== null && set({ advance_by: Math.round(n) })} />
            </div>
            <CheckField label={t("var.autoAdvance")} checked={v.auto_advance} onChange={(b) => set({ auto_advance: b })} />
            <p className="hint">{t("var.autoAdvanceHint")}</p>
            <div className="tx-row-btns">
              <button className="btn" onClick={() => void p.run(advance(-1)).catch(p.report)} title={t("var.prevTitle")}>
                {t("var.prev")}
              </button>
              <button className="btn" onClick={() => void p.run(advance(1)).catch(p.report)} title={t("var.nextTitle")}>
                {t("var.next")}
              </button>
              <button className="btn" onClick={() => set({ reset: true })} title={t("var.resetTitle")}>
                {t("var.reset")}
              </button>
            </div>

            <h4>{t("var.testHead")}</h4>
            <div className="tx-test-row">
              <NumberField label={t("var.row")} value={testValue} allowEmpty placeholder={String(v.current)} step={1} digits={0} onCommit={(n) => setTestValue(n === null ? null : Math.round(n))} />
              <button className="btn" onClick={() => void runTest(testValue ?? undefined)} title={t("var.testTitle")}>
                {t("var.test")}
              </button>
            </div>
            {can(TXT.contactSheet) && (
              <div className="tx-row-btns">
                {/* The contact sheet (text/ContactSheet.tsx), opened by templates/useTemplateUi.tsx. */}
                <button className="btn" onClick={() => window.dispatchEvent(new CustomEvent("openetch:contact-sheet"))} title={t("sheet.openTitle")}>
                  <Icon name="contactSheet" size={16} /> {t("sheet.menu")}
                </button>
              </div>
            )}
            {test && (
              <div className="tx-test" role="status">
                <div className="tx-test-head">{t("var.testFor", { value: test.value })}</div>
                {test.items.length === 0 ? (
                  <div className="hint">{t("var.testNone")}</div>
                ) : (
                  <table>
                    <tbody>
                      {test.items.map((i) => (
                        <tr key={i.id}>
                          <td>{name(i.id)}</td>
                          <td className="tx-shown-v">{i.shown || "∅"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            )}

            <h4>{t("var.csvHead")}</h4>
            {csv ? (
              <>
                <p className="hint">{t("var.csvInfo", { name: csv.name, rows: csv.rows.length })}</p>
                {row && (
                  <div className="tx-csv-row" title={t("var.csvRowTitle")}>
                    {row.map((cell, i) => (
                      <span key={i}>
                        <code>%{i}</code> {cell || "∅"}
                      </span>
                    ))}
                  </div>
                )}
              </>
            ) : (
              <p className="hint">{t("var.csvNone")}</p>
            )}
            <div className="tx-row-btns">
              <button className="btn" onClick={() => fileRef.current?.click()} disabled={!can(TXT.loadCsv)}>
                {t("var.csvLoad")}
              </button>
              <button className="btn" onClick={() => void p.run(clearCsv()).catch(p.report)} disabled={!csv}>
                {t("var.csvClear")}
              </button>
            </div>
            <input ref={fileRef} type="file" accept=".csv,.txt,text/csv,text/plain" hidden onChange={(e) => void onFile(e)} />
          </>
        )}
      </div>
    </Dock>
  );
}
