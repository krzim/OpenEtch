// Barcodes (LightBurn's Create Bar Code / Bar Code Properties): the form shared by the create dialog and the
// Properties section of a selected barcode. Every setting goes to the engine as `shape.barcode` (with `id` to redraw
// an existing code in place: its layer, name and placement stay).

import { useEffect, useState } from "react";
import { api } from "../api/client.ts";
import { Modal, NumberField, CheckField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { TEXT_MODES, barcodeKind, barcodeKinds, barcodeKindsFrom, setEngineBarcodeKinds, type BarcodeKind, type BarcodeParams, type TextMode } from "./textModel.ts";
import { VarHelp } from "./VarHelp.tsx";

export const NEW_BARCODE: BarcodeParams = { kind: "qr", data: "https://example.com", module_mm: 0.5, quiet_zone: 2, invert: false, show_text: false };

/** The barcode kinds the engine offers (read once from GET /api/commands; new types appear as the engine adds them). */
let kindsLoad: Promise<BarcodeKind[]> | null = null;
function useBarcodeKinds(): BarcodeKind[] {
  const [kinds, setKinds] = useState(barcodeKinds);
  useEffect(() => {
    let live = true;
    kindsLoad ??= api
      .commands()
      .then((list) => {
        const spec = list.find((c) => c.id === "shape.barcode");
        const k = spec ? barcodeKindsFrom(spec.input_schema) : null;
        setEngineBarcodeKinds(k);
        return barcodeKinds();
      })
      .catch(() => barcodeKinds());
    void kindsLoad.then((k) => live && setKinds(k));
    return () => {
      live = false;
    };
  }, []);
  return kinds;
}

/** The fields; `onChange` gets the changed parameters (the Properties section sends each change at once). */
export function BarcodeForm(p: { value: BarcodeParams; onChange: (v: BarcodeParams) => void; live?: boolean }) {
  const t = useT();
  const kinds = useBarcodeKinds();
  const v = p.value;
  const kind = barcodeKind(v.kind);
  const set = (c: Partial<BarcodeParams>) => p.onChange({ ...v, ...c });
  const mode: TextMode = v.variable?.mode ?? "normal";
  const [data, setData] = useState<{ base: string; text: string } | null>(null);
  const shownData = data && data.base === v.data ? data.text : v.data;
  const commitData = () => {
    if (data && data.text !== v.data && data.text.length > 0) set({ data: data.text });
    setData(null);
  };
  return (
    <div className="tx-form">
      <label className="field">
        <span className="field-label">{t("barcode.kind")}</span>
        <span className="field-input">
          <select value={v.kind} onChange={(e) => set({ kind: e.target.value, error_correction: null })}>
            {!kind && <option value={v.kind}>{v.kind}</option>}
            <optgroup label={t("barcode.twoD")}>
              {kinds.filter((k) => k.twoD).map((k) => (
                <option key={k.id} value={k.id}>
                  {k.label}
                </option>
              ))}
            </optgroup>
            <optgroup label={t("barcode.oneD")}>
              {kinds.filter((k) => !k.twoD).map((k) => (
                <option key={k.id} value={k.id}>
                  {k.label}
                </option>
              ))}
            </optgroup>
          </select>
        </span>
      </label>
      <label className="field tx-stack">
        <span className="field-label">{t("barcode.data")}</span>
        <textarea
          className="tx-area"
          rows={2}
          value={shownData}
          spellCheck={false}
          onChange={(e) => (p.live ? setData({ base: v.data, text: e.target.value }) : set({ data: e.target.value }))}
          onBlur={p.live ? commitData : undefined}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (p.live && e.key === "Enter" && (e.ctrlKey || e.metaKey)) (e.target as HTMLTextAreaElement).blur();
          }}
        />
      </label>
      <NumberField label={kind?.twoD === false ? t("barcode.barWidth") : t("barcode.module")} unit="mm" value={v.module_mm ?? 0.5} min={0.001} max={100} digits={3} onCommit={(n) => n !== null && set({ module_mm: n })} />
      {kind?.twoD === false && <NumberField label={t("barcode.height")} unit="mm" value={v.height_mm ?? 15} min={0.01} max={2000} digits={2} onCommit={(n) => n !== null && set({ height_mm: n })} />}
      <CheckField label={t("barcode.invert")} checked={!!v.invert} onChange={(b) => set({ invert: b })} />
      {v.invert && <NumberField label={t("barcode.quietZone")} value={v.quiet_zone ?? 2} min={0} max={100} step={1} digits={0} onCommit={(n) => n !== null && set({ quiet_zone: Math.round(n) })} />}
      {kind?.ec && (
        <label className="field">
          <span className="field-label">{t("barcode.ec")}</span>
          <span className="field-input">
            <select value={v.error_correction ?? ""} onChange={(e) => set({ error_correction: e.target.value || null })}>
              <option value="">{t("barcode.ecDefault")}</option>
              {kind.ec.map(([val, label]) => (
                <option key={val} value={val}>
                  {label}
                </option>
              ))}
            </select>
          </span>
        </label>
      )}
      {v.kind === "data_matrix" && <CheckField label={t("barcode.forceSquare")} checked={v.force_square ?? true} onChange={(b) => set({ force_square: b })} />}
      {kind?.twoD === false && <CheckField label={t("barcode.showText")} checked={!!v.show_text} onChange={(b) => set({ show_text: b })} />}
      <label className="field">
        <span className="field-label">{t("text.mode")}</span>
        <span className="field-input">
          <select value={mode} onChange={(e) => set({ variable: { mode: e.target.value as TextMode, offset: v.variable?.offset ?? 0 } })}>
            {TEXT_MODES.map((m) => (
              <option key={m.id} value={m.id}>
                {t(m.key)}
              </option>
            ))}
          </select>
        </span>
      </label>
      {mode !== "normal" && (
        <>
          <NumberField label={t("text.offset")} value={v.variable?.offset ?? 0} step={1} digits={0} onCommit={(n) => n !== null && set({ variable: { mode, offset: Math.round(n) } })} />
          <VarHelp mode={mode} />
        </>
      )}
    </div>
  );
}

/** Create Bar Code: the form in a dialog; OK adds the code at the clicked point. */
export function BarcodeDialog(p: { initial: BarcodeParams; title: string; okLabel: string; onOk: (v: BarcodeParams) => void; onClose: () => void }) {
  const t = useT();
  const [v, setV] = useState<BarcodeParams>(p.initial);
  const ok = v.data.length > 0;
  return (
    <Modal
      title={p.title}
      help="variable-text-and-barcodes#barcodes"
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ok} onClick={() => ok && p.onOk(v)}>
            {p.okLabel}
          </button>
        </>
      }
    >
      <BarcodeForm value={v} onChange={setV} />
      <p className="hint">{t("barcode.hint")}</p>
    </Modal>
  );
}
