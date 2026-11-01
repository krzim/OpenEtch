// "Import LightBurn preferences…" (UX-067), in Preferences and in File > Preferences and bundles: pick a LightBurn
// .lbprefs backup (%LOCALAPPDATA%\LightBurn\backup\prefs) or prefs.ini; the engine previews what it maps (only the keys
// OpenEtch has are read, others are only counted), then Import applies the engine's part (autosave, the active device's
// Optimization defaults; the current preferences are backed up first) and this browser's part (lightburnPrefs.ts).
// The file goes only to this engine; nothing of it is kept in the browser except the mapped settings.

import { useState } from "react";
import { setFrameOptions } from "../components/jobStore.ts";
import { Modal } from "../components/ui.tsx";
import { setDialogDefaults } from "../workspace/dialogDefaults.ts";
import { getPrefs, updatePrefs } from "../workspace/prefs.ts";
import { useT } from "../workspace/usePrefs.ts";
import { filesApi } from "./filesApi.ts";
import { setImportPrefs } from "./filesStore.ts";
import { parseLightBurnPrefsResult, planLightBurnPrefs, planSize, type LightBurnBrowserPlan, type LightBurnPrefsResult } from "./lightburnPrefs.ts";

/** Window events App.tsx and the design layer listen to for the canvas view and the system clipboard choice. */
export const VIEW_PREFS_EVENT = "openetch:view-prefs";
export const SYSTEM_CLIPBOARD_EVENT = "openetch:system-clipboard";

function errText(e: unknown): string {
  if (e && typeof e === "object" && "detail" in e && typeof (e as { detail?: unknown }).detail === "string") return (e as { detail: string }).detail;
  return e instanceof Error ? e.message : String(e);
}

/** Apply the browser part of an import. */
export function applyLightBurnPrefs(plan: LightBurnBrowserPlan): void {
  const { nudge, ...rest } = plan.prefs as Partial<typeof plan.prefs> & { nudge?: Record<string, number> };
  updatePrefs({ ...rest, ...(nudge ? { nudge: { ...getPrefs().nudge, ...nudge } } : {}) });
  if (plan.view.snap || plan.view.viewStyle) window.dispatchEvent(new CustomEvent(VIEW_PREFS_EVENT, { detail: plan.view }));
  if (Object.keys(plan.importPrefs).length) setImportPrefs(plan.importPrefs);
  if (Object.keys(plan.frame).length) setFrameOptions(plan.frame);
  if (plan.systemClipboard !== null) window.dispatchEvent(new CustomEvent(SYSTEM_CLIPBOARD_EVENT, { detail: plan.systemClipboard }));
  if (Object.keys(plan.dialogs).length) setDialogDefaults(plan.dialogs);
}

/** The button and its preview / confirm dialog. */
export function LightBurnPrefsImport(p: { notify: (msg: string, kind?: "info" | "error") => void; onImported?: () => void; className?: string }) {
  const t = useT();
  const [file, setFile] = useState<File | null>(null);
  const [preview, setPreview] = useState<LightBurnPrefsResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pick = (f: File) => {
    setFile(f);
    setPreview(null);
    setError(null);
    setBusy(true);
    filesApi
      .importLightBurnPrefs(f, true)
      .then((r) => setPreview(parseLightBurnPrefsResult(r)))
      .catch((e: unknown) => setError(errText(e)))
      .finally(() => setBusy(false));
  };
  const close = () => {
    setFile(null);
    setPreview(null);
    setError(null);
  };
  const doImport = () => {
    if (!file) return;
    setBusy(true);
    filesApi
      .importLightBurnPrefs(file, false)
      .then((r) => {
        const res = parseLightBurnPrefsResult(r);
        const plan = planLightBurnPrefs(res.browser);
        applyLightBurnPrefs(plan);
        p.notify(t("lbp.imported", { n: res.used.length }));
        p.onImported?.();
        close();
      })
      .catch((e: unknown) => setError(errText(e)))
      .finally(() => setBusy(false));
  };
  const plan = preview ? planLightBurnPrefs(preview.browser) : null;
  return (
    <>
      <label className={p.className ?? "btn"} title={t("lbp.buttonTitle")}>
        {t("lbp.button")}
        <input
          type="file"
          accept=".lbprefs,.ini,application/json"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f) pick(f);
          }}
        />
      </label>
      {file && (
        <Modal
          title={t("lbp.title", { name: file.name })}
          onClose={close}
          footer={
            <>
              {error && <span className="form-error">{error}</span>}
              <button className="btn" onClick={close}>
                {t("common.cancel")}
              </button>
              <button className="btn btn-primary" disabled={busy || !preview || preview.used.length === 0} onClick={doImport}>
                {t("lbp.import")}
              </button>
            </>
          }
        >
          {busy && !preview && <p className="muted">{t("common.loading")}</p>}
          {preview && plan && (
            <>
              <p>{t("lbp.found", { n: preview.used.length, other: preview.otherKeys })}</p>
              <ul className="files-list">
                {preview.autosaveMinutes !== null && <li>{t("lbp.autosave", { m: preview.autosaveMinutes })}</li>}
                {preview.optimization && <li>{t("lbp.optimization")}</li>}
                <li>{t("lbp.browser", { n: planSize(plan) })}</li>
              </ul>
              {preview.used.length > 0 && (
                <details className="files-details">
                  <summary>{t("lbp.keys")}</summary>
                  <p className="hint">{preview.used.join(", ")}</p>
                </details>
              )}
              {preview.rejected.length > 0 && <p className="warn-text">{t("lbp.rejected", { keys: preview.rejected.join(", ") })}</p>}
              <p className="hint">{t("lbp.privacy")}</p>
            </>
          )}
        </Modal>
      )}
    </>
  );
}
