// Colour separation (xTool's Colour Separation, Silhouette's trace by colour): reduce the selected image to a few
// colours (k-means on the engine), trace each into its own fill layer and give each layer the library preset whose
// result colour is nearest, so a logo becomes a coloured MOPA coin in one step. The preview runs on the engine
// (image.color_separate_preview) while the options change; Create is one undo step.

import { useEffect, useRef, useState } from "react";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useLibrary } from "../libs/store.ts";
import { errText } from "../libs/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { materialsApi } from "./api.ts";
import { DEFAULT_SEPARATE, parseSeparation, separateBody, type SepColor, type SeparateOptions } from "./model.ts";
import "./materials.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

export function ColorSeparateDialog(p: { image: string; notify: Notify; report: (e: unknown) => void; onClose: () => void; onCreated?: (ids: string[]) => void }) {
  const t = useT();
  const lib = useLibrary();
  const libs = lib.view?.libraries.map((l) => l.name) ?? [];
  const [o, setO] = useState<SeparateOptions>(() => ({ ...DEFAULT_SEPARATE, library: lib.view?.selected ?? "" }));
  const [pv, setPv] = useState<{ colors: SepColor[]; png: string | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);

  // The live preview, debounced; an older answer never replaces a newer one.
  const key = JSON.stringify({ ...o, skip: [], deleteImage: false });
  useEffect(() => {
    const n = ++seq.current;
    const timer = window.setTimeout(() => {
      materialsApi
        .separate(separateBody(p.image, { ...o, skip: [] }, true))
        .then((r) => {
          if (n !== seq.current) return;
          const s = parseSeparation(r);
          setPv({ colors: s.colors, png: s.png });
          setError(null);
        })
        .catch((e) => n === seq.current && setError(errText(e)));
    }, 250);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, p.image]);

  const traced = (pv?.colors ?? []).filter((c) => !c.background && !o.skip.includes(c.index));
  const create = async () => {
    setBusy(true);
    try {
      const r = (await materialsApi.separate(separateBody(p.image, o, false))) as { created?: unknown; layers?: { swatch?: unknown }[] };
      const layers = Array.isArray(r.layers) ? r.layers : [];
      p.notify(t("mat.sepCreated", { n: layers.length, m: layers.filter((l) => l.swatch).length }));
      p.onCreated?.(Array.isArray(r.created) ? r.created.filter((x): x is string => typeof x === "string") : []);
      p.onClose();
    } catch (e) {
      p.report(new Error(errText(e)));
      setBusy(false);
    }
  };
  const toggle = (i: number) => setO((x) => ({ ...x, skip: x.skip.includes(i) ? x.skip.filter((k) => k !== i) : [...x.skip, i] }));

  return (
    <WsModal
      help="mopa-colour#colour-separation"
      title={t("mat.separateTitle")}
      size="lg"
      onClose={p.onClose}
      onSubmit={() => traced.length > 0 && void create()}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={busy || traced.length === 0} onClick={() => void create()}>
            {t("mat.sepCreate")}
          </button>
        </>
      }
    >
      <div className="mt-sep">
        <div className="mt-sep-opts">
          <NumberField label={t("mat.sepColors")} value={o.colors} min={1} max={16} digits={0} stepper onCommit={(v) => v !== null && setO({ ...o, colors: Math.max(1, Math.min(16, Math.round(v))), skip: [] })} />
          <label className="field">
            <span className="field-label">{t("mat.sepBackground")}</span>
            <span className="field-input">
              <select value={o.background} onChange={(e) => setO({ ...o, background: e.target.value as "auto" | "none", skip: [] })}>
                <option value="auto">{t("mat.sepBgAuto")}</option>
                <option value="none">{t("mat.sepBgNone")}</option>
              </select>
            </span>
          </label>
          <NumberField label={t("mat.sepSpeck")} unit="px" value={o.minSpeckPx} min={0} max={100000} digits={0} allowEmpty placeholder={t("mat.sepSpeckAuto")} onCommit={(v) => setO({ ...o, minSpeckPx: v })} />
          <NumberField label={t("mat.sepSmooth")} value={o.smoothness} min={0} max={1.33} digits={2} onCommit={(v) => v !== null && setO({ ...o, smoothness: v })} />
          <label className="field">
            <span className="field-label">{t("mat.sepLibrary")}</span>
            <span className="field-input">
              <select value={o.library} onChange={(e) => setO({ ...o, library: e.target.value })}>
                <option value="">{t("mat.sepNoLibrary")}</option>
                {libs.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </span>
          </label>
          {o.library && <NumberField label={t("mat.sepDistance")} value={o.maxDistance} min={0} max={200} digits={0} onCommit={(v) => v !== null && setO({ ...o, maxDistance: v })} />}
          {o.library && <CheckField label={t("mat.sepLink")} checked={o.link} onChange={(v) => setO({ ...o, link: v })} />}
          <CheckField label={t("mat.sepDeleteImage")} checked={o.deleteImage} onChange={(v) => setO({ ...o, deleteImage: v })} />
        </div>
        <div className="mt-sep-preview">
          {pv?.png ? <img src={`data:image/png;base64,${pv.png}`} alt={t("mat.sepPreview")} /> : <div className="mt-sep-wait">{error ?? t("mat.sepWorking")}</div>}
          {error && pv && <p className="form-error">{error}</p>}
          <ul className="mt-sep-colors" aria-label={t("mat.sepColors")}>
            {(pv?.colors ?? []).map((c) => (
              <li key={c.index} className={c.background ? "bg" : undefined}>
                <label>
                  <input type="checkbox" disabled={c.background} checked={!c.background && !o.skip.includes(c.index)} onChange={() => toggle(c.index)} aria-label={`${t("mat.sepTrace")} ${c.color}`} />
                  <span className="mt-dot" style={{ background: c.color }} />
                  <span className="mt-sep-share">{t("mat.sepColor", { share: c.share })}</span>
                  {c.background ? (
                    <span className="hint">{t("mat.sepBackgroundTag")}</span>
                  ) : c.swatch ? (
                    <span className="mt-sep-match">
                      <span className="mt-dot" style={{ background: c.swatch.color }} />
                      {t("mat.sepMatch", { name: c.swatch.name, d: c.swatch.distance })}
                    </span>
                  ) : (
                    o.library && <span className="hint">{t("mat.sepNoMatch")}</span>
                  )}
                </label>
              </li>
            ))}
          </ul>
        </div>
      </div>
      <p className="hint">{t("mat.separateHint")}</p>
    </WsModal>
  );
}
