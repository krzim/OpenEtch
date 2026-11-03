// The finish of a material library entry (short list #6): a photo of the result from the iPad / phone camera or a
// file (the colour defaults to its average), the expected result colour, the surface the setting was made for, lens,
// focus / defocus, supplier, lot, date and notes; plus the hue step controls ("one hue warmer / cooler", adapt to
// another surface) that change the preset's settings. Photos upload at once; the other fields with Save. Every edit
// stays in the engine's memory until Save Library, like LightBurn's entry edits.

import { useEffect, useRef, useState, type ChangeEvent } from "react";
import { Icon, NumberField } from "../components/ui.tsx";
import { errText } from "../libs/ui.tsx";
import { takeLibraryView, useLibrary } from "../libs/store.ts";
import { treeEntries, type LibEntry } from "../libs/model.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { formatSpeed } from "../workspace/units.ts";
import { WsModal } from "../workspace/WsModal.tsx";
import { materialsApi } from "./api.ts";
import {
  HUE_PARAMS,
  SURFACES,
  deltaLines,
  finishForm,
  finishFormProblem,
  finishPatch,
  inkOn,
  normalHex,
  parseNudge,
  type EntryFinish,
  type FinishForm,
  type HueParam,
  type NudgeResult,
  type Surface,
} from "./model.ts";
import { encodePhoto } from "./photo.ts";
import { openMaterials, photoUrl, useHueModel } from "./store.ts";
import "./materials.css";
import { HelpLink } from "../guide/HelpLink.tsx";

type Notify = (msg: string, kind?: "info" | "error") => void;

/** The entry's photo (fetched with the Authorization header), or its colour chip. */
export function FinishSwatch(p: { library: string; entry: LibEntry; size?: "small" | "large" }) {
  const f = p.entry.finish ?? null;
  const [url, setUrl] = useState<string | null>(null);
  const taken = f?.photo?.takenUnixS ?? 0;
  const has = !!f?.photo;
  useEffect(() => {
    let alive = true;
    setUrl(null);
    if (has) photoUrl(p.library, p.entry.id, taken).then((u) => alive && setUrl(u), () => undefined);
    return () => {
      alive = false;
    };
  }, [p.library, p.entry.id, taken, has]);
  const color = f?.color ?? null;
  return (
    <span className={`mt-swatch mt-swatch-${p.size ?? "small"}${color ? "" : " empty"}`} style={color ? { background: color, color: inkOn(color) } : undefined} aria-hidden="true">
      {url && <img src={url} alt="" draggable={false} />}
      {url && color && <span className="mt-swatch-chip" style={{ background: color }} />}
    </span>
  );
}

/** Warmer / Cooler and Adapt to surface for one entry: a proposal first, then Apply (writes the preset). */
export function HueControls(p: { library: string; entry: LibEntry; notify: Notify; report: (e: unknown) => void; compact?: boolean }) {
  const t = useT();
  const { speedUnit } = usePrefs();
  const hm = useHueModel();
  const [via, setVia] = useState<HueParam>("auto");
  const [surface, setSurface] = useState<Surface>(p.entry.finish?.surface === "mirror" ? "polished" : "mirror");
  const [proposal, setProposal] = useState<{ what: string; body: Record<string, unknown>; n: NudgeResult } | null>(null);
  const [busy, setBusy] = useState(false);
  const lib = useLibrary();
  const speed = (v: number) => formatSpeed(v, speedUnit);
  const before = p.entry.summary ?? { power_pct: 0, speed_mm_s: 0, frequency_khz: 0 };

  useEffect(() => setProposal(null), [p.entry.id, p.entry.summary]);

  const propose = async (body: Record<string, unknown>, what: string) => {
    setBusy(true);
    try {
      const n = parseNudge(await materialsApi.nudge({ library: p.library, entry: p.entry.id, ...body }));
      setProposal({ what, body, n });
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(false);
    }
  };
  // Each press adds a hue to the proposal; back to 0 drops it.
  const step = (d: number) => {
    const cur = typeof proposal?.body.steps === "number" ? proposal.body.steps : 0;
    const s = cur + d;
    if (s === 0) return setProposal(null);
    void propose({ steps: s, via }, s > 0 ? t("mat.proposalWarmer", { n: s }) : t("mat.proposalCooler", { n: -s }));
  };
  const apply = async () => {
    if (!proposal) return;
    setBusy(true);
    try {
      const r = parseNudge(await materialsApi.nudge({ library: p.library, entry: p.entry.id, ...proposal.body, apply: true }));
      await lib.refresh();
      p.notify(t("mat.applied", { path: p.entry.path, changes: deltaLines(before, r, speed).join(", ") || t("mat.noChange") }));
      setProposal(null);
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(false);
    }
  };
  const lines = proposal ? deltaLines(before, proposal.n, speed) : [];
  return (
    <div className={`mt-hue${p.compact ? " compact" : ""}`}>
      <div className="mt-hue-row">
        <span className="mt-hue-label">{t("mat.hue")}</span> <HelpLink to="mopa-colour#hue-steps" topic={t("mat.hue")} />
        <button type="button" className="btn small mt-warm" disabled={busy || !p.entry.summary} title={t("mat.warmerTitle")} onClick={() => step(1)}>
          <Icon name="flame" size={14} /> {t("mat.warmer")}
        </button>
        <button type="button" className="btn small mt-cool" disabled={busy || !p.entry.summary} title={t("mat.coolerTitle")} onClick={() => step(-1)}>
          <Icon name="snowflake" size={14} /> {t("mat.cooler")}
        </button>
        <label className="mt-inline">
          <span className="sr-only">{t("mat.via")}</span>
          <select className="mini-select" value={via} title={t("mat.via")} onChange={(e) => setVia(e.target.value as HueParam)}>
            {HUE_PARAMS.map((v) => (
              <option key={v} value={v}>
                {t(`mat.via.${v}`)}
              </option>
            ))}
          </select>
        </label>
      </div>
      {!p.compact && (
        <div className="mt-hue-row">
          <span className="mt-hue-label">{t("mat.adapt")}</span> <HelpLink to="mopa-colour#adapt-to-a-surface" topic={t("mat.adapt")} />
          <select className="mini-select" value={surface} aria-label={t("mat.adapt")} onChange={(e) => setSurface(e.target.value as Surface)}>
            {SURFACES.map((s) => (
              <option key={s} value={s}>
                {t(`mat.surface.${s}`)}
              </option>
            ))}
          </select>
          <button type="button" className="btn small" disabled={busy || !p.entry.summary} onClick={() => void propose({ surface }, t("mat.proposalSurface", { surface: t(`mat.surface.${surface}`) }))}>
            {t("mat.adaptGo")}
          </button>
        </div>
      )}
      {proposal && (
        <div className="mt-proposal" role="status">
          <span>{t("mat.proposal", { what: proposal.what, changes: lines.join(", ") || t("mat.noChange") })}</span>
          {proposal.n.warnings.map((w) => (
            <span key={w} className="mt-warn">
              {w}
            </span>
          ))}
          <span className="mt-proposal-btns">
            <button type="button" className="btn small btn-primary" disabled={busy || lines.length === 0} onClick={() => void apply()}>
              {t("mat.apply")}
            </button>
            <button
              type="button"
              className="btn small"
              onClick={() => setProposal(null)}
            >
              {t("common.cancel")}
            </button>
          </span>
        </div>
      )}
      <p className="hint mt-model">
        {t("mat.model", { source: t(hm.model.source === "bracket" ? "mat.model.bracket" : "mat.model.xtool") })}
        {" "}
        <button type="button" className="link-btn" onClick={() => openMaterials({ kind: "hueModel" })}>
          {t("mat.modelEdit")}
        </button>
      </p>
    </div>
  );
}

/** The photo buttons: the camera (`capture`, the rear camera on iPad and phone) and a file. */
function PhotoButtons(p: { disabled: boolean; onFile: (f: File) => void; autoCamera?: boolean }) {
  const t = useT();
  const cam = useRef<HTMLInputElement>(null);
  const file = useRef<HTMLInputElement>(null);
  const opened = useRef(false);
  useEffect(() => {
    // The bracket test's "Use this box" opens the camera straight away on touch devices.
    if (p.autoCamera && !opened.current && window.matchMedia?.("(pointer: coarse)").matches) {
      opened.current = true;
      cam.current?.click();
    }
  }, [p.autoCamera]);
  const pick = (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    e.target.value = "";
    if (f) p.onFile(f);
  };
  return (
    <>
      <input ref={cam} type="file" accept="image/*" capture="environment" hidden onChange={pick} />
      <input ref={file} type="file" accept="image/png,image/jpeg,image/webp,image/heic,image/*" hidden onChange={pick} />
      <button type="button" className="btn small" disabled={p.disabled} onClick={() => cam.current?.click()}>
        <Icon name="camera" size={14} /> {t("mat.takePhoto")}
      </button>
      <button type="button" className="btn small" disabled={p.disabled} onClick={() => file.current?.click()}>
        {t("mat.choosePhoto")}
      </button>
    </>
  );
}

export function FinishDialog(p: { library: string; entryId: string; photo?: boolean; notify: Notify; report: (e: unknown) => void; onClose: () => void }) {
  const t = useT();
  const lib = useLibrary();
  const entry = treeEntries(lib.view?.tree ?? []).find((e) => e.id === p.entryId) ?? null;
  const finish: EntryFinish | null = entry?.finish ?? null;
  const [base, setBase] = useState<FinishForm>(() => finishForm(finish));
  const [form, setForm] = useState<FinishForm>(base);
  const [busy, setBusy] = useState<string | null>(null);
  // A photo upload refreshes the entry; the form keeps what was typed.
  const lastPhoto = useRef(finish?.photo?.takenUnixS ?? 0);
  useEffect(() => {
    const now = finish?.photo?.takenUnixS ?? 0;
    if (now !== lastPhoto.current) {
      lastPhoto.current = now;
      const fresh = finishForm(finish);
      setBase((b) => ({ ...b, color: fresh.color }));
    }
  }, [finish]);

  if (!entry) return null;
  const err = finishFormProblem(form);
  const ok = Object.keys(err).length === 0;
  const patch = finishPatch(base, form);
  const dirty = Object.keys(patch).length > 0;
  const shown = form.color ?? finish?.photo?.average ?? finish?.color ?? null;

  const upload = async (f: File) => {
    setBusy(t("mat.photoBusy"));
    try {
      const { base64, mime } = await encodePhoto(f);
      const r = await materialsApi.finishEdit(p.library, entry.id, { photo_base64: base64, photo_mime: mime });
      takeLibraryView(r);
      await lib.refresh();
      p.notify(t("mat.photoSaved", { path: entry.path }));
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(null);
    }
  };
  const removePhoto = async () => {
    setBusy("…");
    try {
      await materialsApi.finishEdit(p.library, entry.id, { remove_photo: true });
      await lib.refresh();
      p.notify(t("mat.photoRemoved"));
    } catch (e) {
      p.report(new Error(errText(e)));
    } finally {
      setBusy(null);
    }
  };
  const save = async () => {
    if (!ok) return;
    if (!dirty) return p.onClose();
    setBusy("…");
    try {
      await materialsApi.finishEdit(p.library, entry.id, patch);
      await lib.refresh();
      p.notify(t("mat.saved", { path: entry.path }));
      p.onClose();
    } catch (e) {
      p.report(new Error(errText(e)));
      setBusy(null);
    }
  };
  const text = (k: "lens" | "supplier" | "lot", label: string, placeholder?: string) => (
    <label className="field">
      <span className="field-label">{label}</span>
      <span className="field-input">
        <input type="text" value={form[k]} placeholder={placeholder} maxLength={200} onChange={(e) => setForm({ ...form, [k]: e.target.value })} />
      </span>
    </label>
  );

  return (
    <WsModal
      help="mopa-colour#record-a-finish"
      title={t("mat.finishTitle", { path: entry.path })}
      onClose={p.onClose}
      onSubmit={() => void save()}
      size="lg"
      dirty={dirty}
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ok || !!busy} onClick={() => void save()}>
            {t("jobui.ok")}
          </button>
        </>
      }
    >
      <div className="mt-finish">
        <div className="mt-finish-photo">
          <FinishSwatch library={p.library} entry={entry} size="large" />
          {!finish?.photo && <p className="hint">{t("mat.noPhoto")}</p>}
          {finish?.photoStale && <p className="run-hint">{t("mat.photoStale")}</p>}
          <div className="mt-row">
            <PhotoButtons disabled={!!busy} onFile={(f) => void upload(f)} autoCamera={p.photo} />
            {finish?.photo && (
              <button type="button" className="btn small btn-warn" disabled={!!busy} onClick={() => void removePhoto()}>
                {t("mat.removePhoto")}
              </button>
            )}
          </div>
          {busy && <p className="hint">{busy}</p>}
        </div>
        <div className="mt-finish-fields">
          <div className="field">
            <span className="field-label">{t("mat.color")}</span>
            <span className="field-input mt-color">
              <input
                type="color"
                aria-label={t("mat.color")}
                value={normalHex(shown) ?? "#808080"}
                onChange={(e) => setForm({ ...form, color: e.target.value })}
              />
              <code>{shown ?? t("mat.colorNone")}</code>
              {finish?.photo && form.color !== null && (
                <button type="button" className="btn small" onClick={() => setForm({ ...form, color: null })}>
                  {t("mat.colorFromPhoto")}
                </button>
              )}
            </span>
          </div>
          <label className="field">
            <span className="field-label">{t("mat.surface")}</span>
            <span className="field-input">
              <select value={form.surface ?? ""} onChange={(e) => setForm({ ...form, surface: (e.target.value || null) as Surface | null })}>
                <option value="">{t("mat.surfaceNone")}</option>
                {SURFACES.map((s) => (
                  <option key={s} value={s}>
                    {t(`mat.surface.${s}`)}
                  </option>
                ))}
              </select>
            </span>
          </label>
          {text("lens", t("mat.lens"), t("mat.lensPlaceholder"))}
          <NumberField label={t("mat.focus")} unit="mm" value={form.focusMm} min={-100} max={100} digits={2} allowEmpty signed placeholder={t("mat.focusHint")} onCommit={(v) => setForm({ ...form, focusMm: v })} />
          {text("supplier", t("mat.supplier"))}
          {text("lot", t("mat.lot"))}
          <label className="field">
            <span className="field-label">{t("mat.date")}</span>
            <span className="field-input">
              <input type="date" value={form.date} onChange={(e) => setForm({ ...form, date: e.target.value })} />
            </span>
          </label>
          {err.date && <p className="form-error">{err.date}</p>}
          <label className="field mt-notes">
            <span className="field-label">{t("mat.notes")}</span>
            <span className="field-input">
              <textarea rows={3} value={form.notes} maxLength={4000} onChange={(e) => setForm({ ...form, notes: e.target.value })} />
            </span>
          </label>
        </div>
      </div>
      <HueControls library={p.library} entry={entry} notify={p.notify} report={p.report} />
      <p className="hint">{t("mat.finishHint")}</p>
    </WsModal>
  );
}
