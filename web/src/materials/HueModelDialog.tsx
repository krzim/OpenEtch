// The active device's hue step model (UI only, like the other device settings): how far one MOPA hue moves power,
// speed or frequency, and the surface offsets. Seeded from xTool's published model; bracket tests refine it.

import { useState } from "react";
import { NumberField } from "../components/ui.tsx";
import { errText } from "../libs/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { materialsApi } from "./api.ts";
import { parseHueModel, type HueModel } from "./model.ts";
import { hueModel, useHueModel } from "./store.ts";

export function HueModelDialog(p: { notify: (m: string) => void; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const hm = useHueModel();
  const [m, setM] = useState<HueModel>(hm.model);
  const [busy, setBusy] = useState(false);
  const put = async (model: HueModel | null) => {
    setBusy(true);
    try {
      const r = (await materialsApi.hueModelEdit(model as unknown as Record<string, unknown> | null)) as { model?: unknown; calibrated?: unknown };
      hueModel.set({ model: parseHueModel(r.model), calibrated: r.calibrated === true, loaded: true });
      p.notify(t("mat.modelSaved"));
      p.onClose();
    } catch (e) {
      p.report(new Error(errText(e)));
      setBusy(false);
    }
  };
  const num = (k: keyof HueModel, label: string, unit: string, digits: number) => (
    <NumberField label={label} unit={unit} value={m[k] as number} min={0} digits={digits} onCommit={(v) => v !== null && setM({ ...m, [k]: k === "big_jump_hues" ? Math.max(1, Math.round(v)) : v })} />
  );
  return (
    <WsModal
      help="mopa-colour#the-hue-step-model"
      title={t("mat.modelTitle")}
      onClose={p.onClose}
      onSubmit={() => void put(m)}
      footer={
        <>
          <button className="btn" disabled={busy} onClick={() => void put(null)}>
            {t("mat.modelReset")}
          </button>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void put(m)}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      {num("power_pct", t("mat.modelPower"), "%", 2)}
      {num("speed_mm_s", t("mat.modelSpeed"), "mm/s", 1)}
      {num("frequency_khz", t("mat.modelFreq"), "kHz", 1)}
      {num("frequency_from_khz", t("mat.modelFreqFrom"), "kHz", 0)}
      {num("min_pulse_ns", t("mat.modelPulse"), "ns", 0)}
      {num("big_jump_hues", t("mat.modelBig"), "", 0)}
      {num("brushed_power_pct", t("mat.modelBrushed"), "%", 2)}
      {num("polished_power_pct", t("mat.modelPolished"), "%", 2)}
      {num("mirror_power_pct", t("mat.modelMirror"), "%", 2)}
      <p className="hint">{t("mat.model", { source: t(m.source === "bracket" ? "mat.model.bracket" : "mat.model.xtool") })}</p>
      <p className="hint">{t("mat.modelHint")}</p>
    </WsModal>
  );
}
