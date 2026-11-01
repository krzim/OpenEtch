// LightBurn's Image mode settings for a layer that holds images: dither mode, line interval / DPI (editable either
// way), negative, pass-through, dot width correction, scan angle / increment / bidirectional, halftone cells and angle
// (halftone only), min power (grayscale only); each edit is one `layer.update { settings: { image: {...} } }`. Plus
// the "Adjust image…" dialog for the layer's images (gamma, contrast, brightness, enhance radius / amount, denoise,
// stored per image in `geometry.adjust`) with a live preview. The dithered preview (in the dialog and on the canvas)
// is rendered client-side by api/image.ts at the layer's line interval; it is approximate (no scan angle or dot width
// correction). The engine plans the real job.

import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { Invocation } from "../api/client.ts";
import {
  DEFAULT_ADJUST,
  DITHER_MODES,
  IMAGE_ADJUST_CMD,
  adjustFromPreset,
  adjustImage,
  adjustOf,
  changedAdjust,
  dpiFromInterval,
  grayFromRgba,
  grayToRgba,
  imageAdjust,
  imagePresetBody,
  imagePresetList,
  imageSettingsOf,
  imageUpdate,
  imageView,
  intervalFromDpi,
  previewSize,
  processImage,
  type ImageAdjust,
  type ImagePatch,
  type ImagePreset,
  type ImageSettings as ImageSettingsT,
} from "../api/image.ts";
import type { DocObject, GDocument, Geometry, Layer } from "../api/types.ts";
import { CheckField, Modal, NumberField, fmt } from "./ui.tsx";
import { Slice3dSettings } from "../files/Slice3d.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { MsgKey } from "../workspace/i18n.ts";
import { libsApi } from "../libs/api.ts";
import { ConfirmDialog, FilePick, NameDialog, errText, saveDownload } from "../libs/ui.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
/** An image's geometry as the engine sends it (`adjust` only when it changes something). */
type ImageGeometry = Omit<Extract<Geometry, { kind: "image" }>, "kind"> & { adjust?: Partial<ImageAdjust> };

/** Longest side of the dithered bitmap drawn on the canvas, and of the Adjust image dialog's preview. */
const CANVAS_MAX_PX = 1600;
const DIALOG_MAX_PX = 700;

/** The layer's Image section; renders nothing unless the layer holds images. */
export function ImageSettings({
  layer,
  doc,
  run,
  report,
  has,
}: {
  layer: Layer;
  doc?: GDocument | null;
  run: Run;
  report: (e: unknown) => void;
  has?: (cmd: string) => boolean;
}) {
  const t = useT();
  const [adjusting, setAdjusting] = useState(false);
  const images = doc ? doc.objects.filter((o) => o.layer === layer.id && o.geometry.kind === "image") : [];
  if (!doc || images.length === 0) return null;
  const s = imageSettingsOf(layer.settings);
  const set = (p: ImagePatch) => void run(imageUpdate(layer.id, p)).catch(report);
  const num = (f: (v: number) => ImagePatch) => (v: number | null) => v !== null && set(f(v));
  const dpi = dpiFromInterval(s.interval_mm);
  const mode = DITHER_MODES.find((m) => m.value === s.dither);
  const adjusted = images.filter((o) => changedAdjust(adjustOf(o.geometry as ImageGeometry)).length > 0).length;
  return (
    <>
      <h4>
        Image · {images.length} image{images.length === 1 ? "" : "s"}
      </h4>
      <label className="field">
        <span className="field-label">{t("img.dither")}</span>
        <span className="field-input">
          <select value={s.dither} title={mode?.hint} onChange={(e) => set({ dither: e.target.value })}>
            {!mode && <option value={s.dither}>{s.dither}</option>}
            {DITHER_MODES.map((m) => (
              <option key={m.value} value={m.value} title={m.hint}>
                {m.label}
              </option>
            ))}
          </select>
        </span>
      </label>
      {mode && <p className="hint">{mode.hint}.</p>}
      <div className="field-grid">
        <NumberField
          label={t("img.interval")}
          unit="mm"
          value={s.interval_mm}
          min={0.001}
          max={10}
          digits={4}
          disabled={s.pass_through}
          onCommit={num((interval_mm) => ({ interval_mm }))}
        />
        <NumberField
          label={t("img.dpi")}
          value={dpi}
          min={2.54}
          max={25400}
          digits={1}
          disabled={s.pass_through}
          // The engine sets the interval to exactly 25.4 / dpi.
          onCommit={num((dpi) => (intervalFromDpi(dpi) === null ? {} : { dpi }))}
        />
      </div>
      <CheckField label={t("img.negative")} checked={s.negative} onChange={(negative) => set({ negative })} />
      <CheckField label={t("img.passThrough")} checked={s.pass_through} onChange={(pass_through) => set({ pass_through })} />
      {s.pass_through && <p className="hint">{t("img.engravesTheImageSOwnPixels")}</p>}
      <NumberField
        label={t("img.dotWidth")}
        unit="mm"
        value={s.dot_width_mm}
        min={0}
        max={Math.max(0, s.interval_mm)}
        digits={4}
        onCommit={num((dot_width_mm) => ({ dot_width_mm }))}
      />
      <NumberField label={t("img.scanAngle")} unit="°" value={s.angle_deg} digits={2} onCommit={num((angle_deg) => ({ angle_deg }))} />
      <NumberField
        label={t("img.increment")}
        unit="°/pass"
        value={s.angle_increment_deg}
        digits={2}
        onCommit={num((angle_increment_deg) => ({ angle_increment_deg }))}
      />
      <CheckField label={t("img.bidirectional")} checked={s.bidirectional} onChange={(bidirectional) => set({ bidirectional })} />
      {s.dither === "halftone" && (
        <>
          <NumberField
            label={t("img.cells")}
            unit="per inch"
            value={s.halftone_cells_per_inch}
            min={1}
            max={1000}
            digits={1}
            onCommit={num((halftone_cells_per_inch) => ({ halftone_cells_per_inch }))}
          />
          <NumberField
            label={t("img.halftoneAngle")}
            unit="°"
            value={s.halftone_angle_deg}
            digits={2}
            onCommit={num((halftone_angle_deg) => ({ halftone_angle_deg }))}
          />
        </>
      )}
      {s.dither === "grayscale" && (
        <>
          <NumberField
            label={t("img.minPower")}
            unit="%"
            value={s.min_power_pct}
            min={0}
            max={Math.max(0, layer.settings.power_pct)}
            digits={1}
            onCommit={num((min_power_pct) => ({ min_power_pct }))}
          />
          <p className="hint">The lightest engraved dot; the darkest uses the layer's power ({fmt(layer.settings.power_pct, 1)} %). Pure white never fires.</p>
        </>
      )}
      <Slice3dSettings layer={layer} run={run} report={report} has={has} />
      <div className="btn-row">
        <button className="btn" title={t("img.gammaContrastBrightnessAndEnhanceOf")} onClick={() => setAdjusting(true)}>
          {t("img.adjustImage")}
        </button>
        {adjusted > 0 && <span className="hint">{adjusted === images.length ? (adjusted === 1 ? t("rest.adjusted") : t("img.allAdjusted")) : t("rest.adjustedOf", { n: adjusted, total: images.length })}</span>}
      </div>
      <ImageViewToggle />
      <p className="hint">{t("img.theDitheredViewIsAPreview")}</p>
      {adjusting && (
        <AdjustImageDialog
          layer={layer}
          images={images}
          canApply={has?.(IMAGE_ADJUST_CMD) ?? false}
          onClose={() => setAdjusting(false)}
          onApply={(ids, adjust) => {
            setAdjusting(false);
            run(imageAdjust(ids, adjust)).catch(report);
          }}
        />
      )}
    </>
  );
}

/** Original / dithered switch for the images on the canvas (a per-browser preference). */
export function ImageViewToggle() {
  const t = useT();
  const { view } = useSyncExternalStore(imageView.subscribe, imageView.get);
  return (
    <div className="segmented" role="radiogroup" aria-label={t("img.imagesOnTheCanvas")}>
      {(["original", "dithered"] as const).map((v) => (
        <button
          key={v}
          role="radio"
          aria-checked={view === v}
          className={view === v ? "active" : ""}
          title={v === "original" ? t("img.showImagesOnTheCanvasAs") : t("img.showImagesOnTheCanvasAs2")}
          onClick={() => imageView.setView(v)}
        >
          {v === "original" ? t("rest.original") : t("rest.dithered")}
        </button>
      ))}
    </div>
  );
}

/** Checkbox form of the view switch, for the toolpath card; renders nothing when the document has no images. */
export function DitheredImagesOption({ doc }: { doc: GDocument | null }) {
  const t = useT();
  const { view } = useSyncExternalStore(imageView.subscribe, imageView.get);
  if (!doc?.objects.some((o) => o.geometry.kind === "image")) return null;
  return (
    <label title={t("img.showImagesOnTheCanvasAs")}>
      <input type="checkbox" checked={view === "dithered"} onChange={(e) => imageView.setView(e.target.checked ? "dithered" : "original")} /> {t("rest.ditheredImages")}
    </label>
  );
}

/** The Adjust Image fields with doc::ImageAdjust's ranges (sliders cover the useful part; the numbers take all). */
const ADJUST_FIELDS: { key: keyof ImageAdjust; label: MsgKey; min: number; max: number; slider: [number, number]; step: number; unit?: string }[] = [
  { key: "gamma", label: "img.gamma", min: 0.01, max: 10, slider: [0.1, 4], step: 0.01 },
  { key: "contrast", label: "img.contrast", min: -100, max: 100, slider: [-100, 100], step: 1, unit: "%" },
  { key: "brightness", label: "img.brightness", min: -100, max: 100, slider: [-100, 100], step: 1, unit: "%" },
  { key: "enhance_radius", label: "img.enhanceRadius", min: 0, max: 100, slider: [0, 20], step: 1, unit: "px" },
  { key: "enhance_amount", label: "img.enhanceAmount", min: 0, max: 10000, slider: [0, 1000], step: 10, unit: "%" },
  { key: "denoise", label: "img.denoise", min: 0, max: 255, slider: [0, 64], step: 1 },
];

/**
 * LightBurn's Adjust Image dialog for one image of a layer (or all of them): the six adjustments as sliders and
 * numbers, with the adjusted greyscale and the dithered result side by side. While open, the canvas shows the draft
 * dithered on the image(s) it applies to; Apply sends one `image.adjust`, Cancel changes nothing.
 */
export function AdjustImageDialog({
  layer,
  images,
  canApply,
  onClose,
  onApply,
}: {
  layer: Layer;
  images: DocObject[];
  canApply: boolean;
  onClose: () => void;
  /** `image`: the layer's image settings of a picked preset that holds them (applied with the adjustments). */
  onApply: (ids: string[], a: ImageAdjust, image?: Partial<ImageSettingsT>) => void;
}) {
  const t = useT();
  const [presetImage, setPresetImage] = useState<Partial<ImageSettingsT> | null>(null);
  const [pick, setPick] = useState(images[0]?.id ?? "");
  const obj = images.find((o) => o.id === pick) ?? images[0];
  const geom = obj?.geometry.kind === "image" ? (obj.geometry as ImageGeometry) : null;
  const current = adjustOf(geom);
  const [draft, setDraft] = useState<ImageAdjust>(current);
  const [all, setAll] = useState(false);
  const targets = useMemo(() => (all ? images.map((o) => o.id) : obj ? [obj.id] : []), [all, images, obj]);
  const targetKey = targets.join(",");

  // The canvas follows the draft while the dialog is open.
  useEffect(() => {
    for (const id of targets) imageView.setDraft(id, draft);
    return () => {
      for (const id of targets) imageView.setDraft(id, null);
    };
    // `targetKey` stands for `targets`.
  }, [targetKey, draft]);

  const settings = imageSettingsOf(layer.settings);
  const preview = usePreview(geom, settings, draft, layer.settings.power_pct, DIALOG_MAX_PX, true, 80);
  const changed = all || !!presetImage || (Object.keys(DEFAULT_ADJUST) as (keyof ImageAdjust)[]).some((k) => draft[k] !== current[k]);
  const set = (k: keyof ImageAdjust, v: number) => setDraft({ ...draft, [k]: v });
  return (
    <Modal
      title={t("img.adjustImageV1", { v1: layer.name })}
      wide
      onClose={onClose}
      footer={
        <>
          <button className="btn img-adjust-reset" onClick={() => setDraft(DEFAULT_ADJUST)}>
            {t("img.reset")}
          </button>
          <button className="btn" onClick={onClose}>
            {t("img.cancel")}
          </button>
          <button
            className="btn btn-primary"
            disabled={!changed || !canApply || targets.length === 0}
            title={canApply ? undefined : t("img.thisEngineCannotStoreImageAdjustments")}
            onClick={() => onApply(targets, draft, presetImage ?? undefined)}
          >
            {t("img.apply")}
          </button>
        </>
      }
    >
      {images.length > 1 && (
        <>
          <label className="field">
            <span className="field-label">{t("img.image")}</span>
            <span className="field-input">
              <select
                value={obj?.id ?? ""}
                onChange={(e) => {
                  setPick(e.target.value);
                  const o = images.find((x) => x.id === e.target.value);
                  if (o && !all) setDraft(adjustOf(o.geometry as ImageGeometry));
                }}
              >
                {images.map((o) => (
                  <option key={o.id} value={o.id}>
                    {o.name || o.id}
                  </option>
                ))}
              </select>
            </span>
          </label>
          <CheckField label={t("img.applyToAllV1Images", { v1: images.length })} checked={all} onChange={setAll} />
        </>
      )}
      <AdjustPresets
        draft={draft}
        layerImage={settings}
        pendingImage={!!presetImage}
        onPick={(pr) => {
          setDraft(adjustFromPreset(pr, draft));
          setPresetImage(pr.image ?? null);
        }}
      />
      <div className="img-adjust">
        <div className="img-adjust-fields">
          {ADJUST_FIELDS.map((f) => (
            <div key={f.key} className="img-slider">
              <NumberField label={t(f.label)} unit={f.unit} value={draft[f.key]} min={f.min} max={f.max} digits={2} onCommit={(v) => v !== null && set(f.key, v)} />
              <input
                type="range"
                aria-label={t(f.label)}
                min={f.slider[0]}
                max={f.slider[1]}
                step={f.step}
                value={Math.min(f.slider[1], Math.max(f.slider[0], draft[f.key]))}
                onChange={(e) => set(f.key, Number(e.target.value))}
              />
            </div>
          ))}
          <p className="hint">{t("rest.adjustHint")}</p>
          {!canApply && <p className="warn-text">{t("img.thisEngineCannotStoreImageAdjustments")}</p>}
        </div>
        <div className="img-adjust-previews">
          <figure>
            {preview?.gray ? <img src={preview.gray} alt={t("img.adjustedGreyscale")} /> : <div className="img-placeholder">{t("img.rendering")}</div>}
            <figcaption>{settings.negative ? t("rest.adjustedNegative") : t("rest.adjusted")}</figcaption>
          </figure>
          <figure>
            {preview?.url ? <img src={preview.url} alt={t("img.ditheredPreview")} className="pixelated" /> : <div className="img-placeholder">{t("img.rendering")}</div>}
            <figcaption>
              {settings.pass_through ? t("rest.passThrough") : (DITHER_MODES.find((m) => m.value === settings.dither)?.label ?? settings.dither)}
              {preview ? ` · ${preview.width}×${preview.height} px${preview.capped ? ` ${t("rest.reduced")}` : ""}` : ""}
            </figcaption>
          </figure>
        </div>
      </div>
    </Modal>
  );
}

/**
 * LightBurn's Adjust Image presets: pick one to load its adjustments (and, when it holds them, the layer's image
 * settings, applied with Apply), Save the current values (optionally with the layer's image settings), Delete, and
 * Import / Export the engine's preset JSON (LightBurn's own preset file is undocumented and not read).
 */
function AdjustPresets(p: { draft: ImageAdjust; layerImage: ImageSettingsT; pendingImage: boolean; onPick: (pr: ImagePreset) => void }) {
  const t = useT();
  const [list, setList] = useState<ImagePreset[] | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [picked, setPicked] = useState("");
  const [withLayer, setWithLayer] = useState(false);
  const [dlg, setDlg] = useState<"save" | "delete" | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; error?: boolean } | null>(null);
  const reload = () =>
    libsApi
      .imagePresets()
      .then((r) => setList(imagePresetList(r)))
      .catch(() => setUnsupported(true));
  useEffect(() => {
    void reload();
  }, []);
  if (unsupported) return null;
  const act = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      setMsg({ text: await fn() });
    } catch (e) {
      setMsg({ text: errText(e), error: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="img-presets">
      <label className="field">
        <span className="field-label">{t("img.preset")}</span>
        <span className="field-input">
          <select
            value={picked}
            disabled={list === null}
            onChange={(e) => {
              setPicked(e.target.value);
              const pr = list?.find((x) => x.name === e.target.value);
              if (pr) p.onPick(pr);
            }}
          >
            <option value="">{list === null ? t("common.loading") : list.length ? t("img.presetPick") : t("img.presetNone")}</option>
            {(list ?? []).map((x) => (
              <option key={x.name} value={x.name}>
                {x.image ? t("img.presetWithLayer", { name: x.name }) : x.name}
              </option>
            ))}
          </select>
        </span>
      </label>
      <div className="img-preset-actions">
        <button className="btn small" disabled={busy || list === null} onClick={() => setDlg("save")}>
          {t("img.presetSave")}
        </button>
        <button className="btn small" disabled={busy || !picked} onClick={() => setDlg("delete")}>
          {t("img.presetDelete")}
        </button>
        <FilePick
          className="btn small"
          label={t("img.presetImport")}
          accept=".json,application/json"
          disabled={busy}
          onFile={(f) =>
            void act(async () => {
              let body: unknown;
              try {
                body = JSON.parse(await f.text());
              } catch {
                throw new Error(t("img.presetNotJson"));
              }
              const r = (await libsApi.imagePresetImport(body)) as { imported?: unknown };
              await reload();
              const names = Array.isArray(r?.imported) ? r.imported.map(String) : [];
              return t("img.presetImported", { n: names.length, names: names.slice(0, 5).join(", ") });
            })
          }
        />
        <button
          className="btn small"
          disabled={busy || !list?.length}
          onClick={() =>
            void act(async () => {
              const blob = new Blob([JSON.stringify({ presets: list ?? [] }, null, 2)], { type: "application/json" });
              const name = await saveDownload(Promise.resolve({ blob, filename: null }), "openetch-image-presets.json");
              return t("img.presetExported", { name });
            })
          }
        >
          {t("img.presetExport")}
        </button>
      </div>
      <CheckField label={t("img.presetIncludeLayer")} checked={withLayer} onChange={setWithLayer} />
      {p.pendingImage && <p className="hint">{t("img.presetLayerPending")}</p>}
      {msg && <p className={msg.error ? "form-error" : "hint"}>{msg.text}</p>}
      {dlg === "save" && (
        <NameDialog
          title={t("img.presetSaveTitle")}
          label={t("img.presetName")}
          initial={picked}
          ok={t("img.presetSave")}
          note={t("img.presetSaveNote")}
          problem={(n) => (!n.trim() ? t("img.presetNameEmpty") : [...n.trim()].length > 100 ? t("img.presetNameLong") : null)}
          onClose={() => setDlg(null)}
          onOk={(name) => {
            setDlg(null);
            void act(async () => {
              await libsApi.imagePresetSave(name, imagePresetBody(p.draft, withLayer ? p.layerImage : null));
              await reload();
              setPicked(name);
              return t("img.presetSaved", { name });
            });
          }}
        />
      )}
      {dlg === "delete" && (
        <ConfirmDialog
          title={t("img.presetDelete")}
          ok={t("img.presetDelete")}
          danger
          onClose={() => setDlg(null)}
          onOk={() => {
            setDlg(null);
            const name = picked;
            void act(async () => {
              await libsApi.imagePresetDelete(name);
              setPicked("");
              await reload();
              return t("img.presetDeleted", { name });
            });
          }}
        >
          <p>{t("img.presetDeleteAsk", { name: picked })}</p>
        </ConfirmDialog>
      )}
    </div>
  );
}

/**
 * The dithered data URL for image object `id` on the canvas, or null to draw the original: when the view is
 * "dithered" or the Adjust image dialog has a draft for it. Keeps the last result while a new one renders.
 */
export function useDitheredHref(id: string, g: ImageGeometry, layer: Layer | undefined): string | null {
  const st = useSyncExternalStore(imageView.subscribe, imageView.get);
  const draft = st.drafts.get(id);
  const on = st.view === "dithered" || !!draft;
  const settings = imageSettingsOf(layer?.settings);
  const r = usePreview(on ? g : null, settings, draft ?? adjustOf(g), layer?.settings.power_pct ?? 100, CANVAS_MAX_PX, false, draft ? 120 : 0);
  return on ? (r?.url ?? null) : null;
}

interface PreviewResult {
  url: string;
  gray?: string;
  width: number;
  height: number;
  capped: boolean;
}

/** Renders the preview of `g` (debounced by `delayMs`), keeping the previous result until the next one is ready. */
function usePreview(
  g: ImageGeometry | null,
  s: ImageSettingsT,
  a: ImageAdjust,
  powerPct: number,
  maxPx: number,
  gray: boolean,
  delayMs: number,
): PreviewResult | null {
  const [res, setRes] = useState<PreviewResult | null>(null);
  const key = JSON.stringify([s, a]);
  useEffect(() => {
    if (!g) return;
    let live = true;
    const t = window.setTimeout(() => {
      renderPreview(g, s, a, powerPct, maxPx, gray).then(
        (r) => live && setRes(r),
        () => live && setRes(null),
      );
    }, delayMs);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
    // `key` stands for `s` and `a` (new objects on every render).
  }, [g?.data_base64, g?.mime, g?.width_mm, g?.height_mm, key, powerPct, maxPx, gray, delayMs]);
  return g ? res : null;
}

// Decoded bitmaps and finished previews, per image data (a few recent ones).
const decoded = new Map<string, Promise<HTMLImageElement>>();
const rendered = new Map<string, Map<string, Promise<PreviewResult>>>();
const KEEP = 12;

function remember<V>(m: Map<string, V>, k: string, v: V) {
  m.delete(k);
  m.set(k, v);
  while (m.size > KEEP) m.delete(m.keys().next().value as string);
}

function decode(mime: string, b64: string): Promise<HTMLImageElement> {
  let p = decoded.get(b64);
  if (!p) {
    const img = new Image();
    img.src = `data:${mime};base64,${b64}`;
    p = img.decode().then(() => img);
    p.catch(() => decoded.delete(b64));
  }
  remember(decoded, b64, p);
  return p;
}

async function renderPreview(g: ImageGeometry, s: ImageSettingsT, a: ImageAdjust, powerPct: number, maxPx: number, withGray: boolean): Promise<PreviewResult> {
  const key = `${JSON.stringify([s, a])}|${powerPct}|${maxPx}|${withGray}|${g.width_mm}x${g.height_mm}`;
  let byKey = rendered.get(g.data_base64);
  const hit = byKey?.get(key);
  if (hit) return hit;
  const p = (async () => {
    const img = await decode(g.mime, g.data_base64);
    // Pass-through engraves the image's own pixels; otherwise it is resampled to the scan grid.
    const size = s.pass_through ? previewSize(img.naturalWidth, img.naturalHeight, 1, maxPx) : previewSize(g.width_mm, g.height_mm, s.interval_mm, maxPx);
    const pixelMm = g.width_mm / size.width;
    const canvas = document.createElement("canvas");
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) throw new Error("no 2D canvas");
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(img, 0, 0, size.width, size.height);
    const src = grayFromRgba(ctx.getImageData(0, 0, size.width, size.height).data, size.width, size.height);
    // The engine enhances at the image's own resolution: scale the radius to this grid.
    const radiusScale = size.width / Math.max(1, img.naturalWidth);
    const out = processImage(src, s, a, pixelMm, powerPct, radiusScale);
    ctx.putImageData(new ImageData(grayToRgba(out), size.width, size.height), 0, 0);
    const url = canvas.toDataURL("image/png");
    let gray: string | undefined;
    if (withGray) {
      const adj = adjustImage(src, a, radiusScale);
      const bytes = Uint8ClampedArray.from(adj.data, (v) => (s.negative ? 255 - v : v));
      ctx.putImageData(new ImageData(grayToRgba(bytes), size.width, size.height), 0, 0);
      gray = canvas.toDataURL("image/png");
    }
    return { url, gray, ...size };
  })();
  p.catch(() => byKey?.delete(key));
  if (!byKey) byKey = new Map();
  remember(byKey, key, p);
  remember(rendered, g.data_base64, byKey);
  return p;
}
