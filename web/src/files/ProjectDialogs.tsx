// Project files around the document: Restore from autosave / backups (with the autosave settings), templates (open,
// upload a LightBurn .lbt, Save As Template), preferences export / import and their automatic backups, user bundles,
// the project Notes window (LightBurn's Show Notes, Ctrl+Alt+N) and the Open dialog's project thumbnails.

import { useEffect, useRef, useState } from "react";
import { CheckField, Icon, Modal } from "../components/ui.tsx";
import { formatBytes, formatModified, validateProjectName } from "../api/project.ts";
import { useT } from "../workspace/usePrefs.ts";
import {
  backupLists,
  bundleResult,
  fileSettingsOf,
  fileSettingsProblem,
  prefsBackupList,
  templateList,
  templateNameOf,
  type FileSettings,
  type Notes,
  type StoredFile,
  type TemplateEntry,
} from "./fileFormats.ts";
import { filesApi, saveBlob } from "./filesApi.ts";
import { FilePick, NumBox, Segmented, parseBox } from "./FilesUiParts.tsx";
import { LightBurnPrefsImport, applyLightBurnPrefs } from "./LightBurnPrefsImport.tsx";
import { planLightBurnPrefs } from "./lightburnPrefs.ts";
import { errorText } from "../api/errorText.ts";
import { HelpLink } from "../guide/HelpLink.tsx";
import { DESIGN_ONLY } from "../mode.ts";

type Notify = (msg: string, kind?: "info" | "error") => void;

function useLoad<T>(load: () => Promise<unknown>, parse: (r: unknown) => T, deps: unknown[] = []): { data: T | null; error: string | null; reload: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    let alive = true;
    load()
      .then((r) => alive && (setData(parse(r)), setError(null)))
      .catch((e) => alive && (setData(parse(undefined)), setError(errorText(e))));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n, ...deps]);
  return { data, error, reload: () => setN((x) => x + 1) };
}

// --- autosave and backups -----------------------------------------------------------------------------------------

export function BackupsDialog(p: { project: string | null; onRestore: (kind: "autosave" | "backup", file: string) => void; onClose: () => void; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const [scope, setScope] = useState<"project" | "all">(p.project ? "project" : "all");
  const lists = useLoad(() => filesApi.backups(scope === "project" ? p.project : null), backupLists, [scope]);
  const settings = useLoad(() => filesApi.fileSettings(), fileSettingsOf);
  const [draft, setDraft] = useState<Record<keyof FileSettings, string> | null>(null);
  useEffect(() => {
    if (settings.data && !draft) {
      const s = settings.data;
      setDraft({ autosave_minutes: String(s.autosave_minutes), autosave_keep: String(s.autosave_keep), backups_keep: String(s.backups_keep) });
    }
  }, [settings.data, draft]);
  const parsed: FileSettings | null = draft
    ? { autosave_minutes: parseBox(draft.autosave_minutes) ?? -1, autosave_keep: parseBox(draft.autosave_keep) ?? -1, backups_keep: parseBox(draft.backups_keep) ?? -1 }
    : null;
  const problem = parsed ? fileSettingsProblem(parsed) : null;
  const changed = !!(parsed && settings.data && (Object.keys(parsed) as (keyof FileSettings)[]).some((k) => parsed[k] !== settings.data?.[k]));
  const save = () => {
    if (!parsed || problem) return;
    filesApi
      .setFileSettings({ ...parsed })
      .then(() => {
        p.notify(t("files.settingsSaved"));
        settings.reload();
        setDraft(null);
      })
      .catch(p.report);
  };
  const rows = (list: StoredFile[], kind: "autosave" | "backup") =>
    list.length === 0 ? (
      <p className="muted">{kind === "autosave" ? t("files.noAutosaves") : t("files.noBackups")}</p>
    ) : (
      <div className="project-list">
        {list.map((s) => (
          <button key={s.file} className="project-row" onClick={() => p.onRestore(kind, s.file)} title={t("files.restoreHint")}>
            <span className="project-row-name">{s.project ?? t("files.untitled")}</span>
            <span className="project-row-meta">{[formatModified(s.savedUnixS), formatBytes(s.size)].filter(Boolean).join(" · ")}</span>
          </button>
        ))}
      </div>
    );
  const set = (k: keyof FileSettings) => (v: string) => setDraft((d) => (d ? { ...d, [k]: v } : d));
  return (
    <Modal
      help="files-and-projects#autosave-and-backups"
      title={t("files.backupsTitle")}
      onClose={p.onClose}
      wide
      footer={
        <button className="btn" onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      {p.project && (
        <Segmented
          label={t("files.backupsScope")}
          value={scope}
          onChange={setScope}
          options={[
            { value: "project", label: t("files.thisProject", { name: p.project }) },
            { value: "all", label: t("files.allProjects") },
          ]}
        />
      )}
      {lists.error && <p className="form-error">{lists.error}</p>}
      <div className="section-label">{t("files.autosaves")}</div>
      {lists.data ? rows(lists.data.autosaves, "autosave") : <p className="muted">{t("common.loading")}</p>}
      <div className="section-label">{t("files.backups")}</div>
      {lists.data ? rows(lists.data.backups, "backup") : null}
      <p className="hint">{t("files.restoreNote")}</p>
      <div className="section-label">{t("files.autosaveSettings")}</div>
      {draft ? (
        <>
          <div className="files-grid3">
            <NumBox label={t("files.autosaveEvery")} value={draft.autosave_minutes} onChange={set("autosave_minutes")} unit="min" title={t("files.autosaveOff")} />
            <NumBox label={t("files.autosaveKeep")} value={draft.autosave_keep} onChange={set("autosave_keep")} />
            <NumBox label={t("files.backupsKeep")} value={draft.backups_keep} onChange={set("backups_keep")} />
          </div>
          <div className="btn-row">
            {problem && <span className="form-error">{problem}</span>}
            <button className="btn btn-primary" disabled={!changed || !!problem} onClick={save}>
              {t("files.saveSettings")}
            </button>
          </div>
        </>
      ) : (
        <p className="muted">{settings.error ?? t("common.loading")}</p>
      )}
    </Modal>
  );
}

// --- templates ----------------------------------------------------------------------------------------------------

export function TemplatesDialog(p: { onOpen: (name: string) => void; onClose: () => void; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const list = useLoad(() => filesApi.templates(), templateList);
  const pick = useRef<HTMLInputElement>(null);
  const upload = (files: File[]) => {
    void (async () => {
      for (const f of files) {
        const name = templateNameOf(f.name);
        try {
          await filesApi.uploadTemplate(name, f);
          p.notify(t("files.templateUploaded", { name }));
        } catch (e) {
          p.report(e);
        }
      }
      list.reload();
    })();
  };
  return (
    <Modal
      help="templates#save-and-open-templates"
      title={t("files.templatesTitle")}
      onClose={p.onClose}
      footer={
        <>
          <button className="btn" onClick={() => pick.current?.click()}>
            <Icon name="upload" /> {t("files.uploadLbt")}
          </button>
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
        </>
      }
    >
      <FilePick ref={pick} accept=".lbt" multiple onFiles={upload} />
      {list.error && <p className="form-error">{list.error}</p>}
      {list.data === null && <p className="muted">{t("common.loading")}</p>}
      {list.data?.length === 0 && <p className="muted">{t("files.noTemplates")}</p>}
      {list.data && list.data.length > 0 && (
        <div className="project-list">
          {list.data.map((tp: TemplateEntry) => (
            <button key={`${tp.kind}:${tp.name}`} className="project-row" onClick={() => p.onOpen(tp.name)}>
              <span className="project-row-name">
                {tp.name} {tp.kind === "lbt" && <span className="files-badge">.lbt</span>}
              </span>
              <span className="project-row-meta">{[formatModified(tp.modifiedUnixS), formatBytes(tp.size)].filter(Boolean).join(" · ")}</span>
            </button>
          ))}
        </div>
      )}
      <p className="hint">{t("files.templatesHint")}</p>
    </Modal>
  );
}

export function SaveTemplateDialog(p: { initial: string; onSaved: (name: string) => void; onClose: () => void; report: (e: unknown) => void }) {
  const t = useT();
  const [name, setName] = useState(p.initial);
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const trimmed = name.trim();
  const problem = trimmed ? validateProjectName(trimmed) : null;
  const submit = () => {
    if (!trimmed || problem || busy) return;
    setBusy(true);
    filesApi
      .saveTemplate(trimmed, replace)
      .then(() => p.onSaved(trimmed))
      .catch((e: unknown) => {
        if (e && typeof e === "object" && (e as { status?: number }).status === 409) setReplace(true);
        else p.report(e);
      })
      .finally(() => setBusy(false));
  };
  return (
    <Modal
      help="templates#save-and-open-templates"
      title={t("files.saveTemplateTitle")}
      onClose={p.onClose}
      onSubmit={submit}
      footer={
        <>
          {problem && <span className="form-error">{problem}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className={`btn ${replace ? "btn-warn" : "btn-primary"}`} disabled={!trimmed || !!problem || busy} onClick={submit}>
            {replace ? t("files.replace") : t("files.save")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("files.templateName")}</span>
        <span className="field-input">
          <input
            type="text"
            autoFocus
            maxLength={100}
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setReplace(false);
            }}
          />
        </span>
      </label>
      {replace && <p className="hint warn-text">{t("files.templateExists", { name: trimmed })}</p>}
      <p className="hint">{t("files.saveTemplateHint")}</p>
    </Modal>
  );
}

// --- preferences and bundles --------------------------------------------------------------------------------------

/** Bundle parts; material test presets, image presets and art libraries travel in LightBurn bundles (.lbzip) only. */
const ALL_PARTS = ["prefs", "devices", "libraries", "presets", "images", "art"] as const;
/** OpenEtch Design's bundles: preferences and material libraries (it has no LightBurn bundles, and no machine). */
const PARTS: readonly (typeof ALL_PARTS)[number][] = DESIGN_ONLY ? ["prefs", "libraries"] : ALL_PARTS;
const PART_LABELS = {
  prefs: "files.partPrefs",
  devices: "files.partDevices",
  libraries: "files.partLibraries",
  presets: "files.partPresets",
  images: "files.partImages",
  art: "files.partArt",
} as const;

export function PrefsBundlesDialog(p: { onClose: () => void; notify: Notify; report: (e: unknown) => void }) {
  const t = useT();
  const backups = useLoad(() => filesApi.prefsBackups(), prefsBackupList);
  const prefsPick = useRef<HTMLInputElement>(null);
  const bundlePick = useRef<HTMLInputElement>(null);
  const [parts, setParts] = useState<Record<(typeof ALL_PARTS)[number], boolean>>({ prefs: true, devices: true, libraries: true, presets: true, images: true, art: true });
  const [result, setResult] = useState<ReturnType<typeof bundleResult> | null>(null);
  const [confirm, setConfirm] = useState<{ what: string; go: () => void } | null>(null);
  const chosen = PARTS.filter((k) => parts[k]);
  const dl = (pr: Promise<{ blob: Blob; filename: string | null }>, fallback: string) =>
    pr
      .then((d) => {
        const name = d.filename ?? fallback;
        saveBlob(d.blob, name);
        p.notify(t("files.downloaded", { name }));
      })
      .catch(p.report);
  const partLabel = (k: (typeof ALL_PARTS)[number]) => t(PART_LABELS[k]);
  return (
    <Modal
      help="settings#preferences-and-bundles"
      title={t("files.prefsTitle")}
      // Escape with the confirmation open closes only the confirmation.
      onClose={() => (confirm ? setConfirm(null) : p.onClose())}
      wide
      footer={
        <button className="btn" onClick={p.onClose}>
          {t("common.close")}
        </button>
      }
    >
      <FilePick
        ref={prefsPick}
        accept=".oeprefs,.json"
        onFiles={([f]) =>
          setConfirm({
            what: t("files.confirmPrefsImport", { name: f.name }),
            go: () =>
              void filesApi
                .importPrefs(f)
                .then(() => {
                  p.notify(t("files.prefsImported"));
                  backups.reload();
                })
                .catch(p.report),
          })
        }
      />
      <FilePick
        ref={bundlePick}
        accept={DESIGN_ONLY ? ".oebundle,.zip" : ".oebundle,.lbzip,.zip"}
        onFiles={([f]) =>
          setConfirm({
            what: t("files.confirmBundleImport", { name: f.name, parts: chosen.map(partLabel).join(", ") }),
            go: () =>
              void filesApi
                .importBundle(chosen, f)
                .then((r) => {
                  const res = bundleResult(r);
                  // A LightBurn bundle's settings: this browser's part, as Import LightBurn preferences applies it.
                  if (res.browser) applyLightBurnPrefs(planLightBurnPrefs(res.browser));
                  setResult(res);
                  p.notify(t("files.bundleImported"));
                  backups.reload();
                })
                .catch(p.report),
          })
        }
      />
      <div className="section-label">{t("files.engineprefs")}</div>
      <div className="btn-row">
        <button className="btn" onClick={() => dl(filesApi.exportPrefs(), "openetch-prefs.oeprefs")}>
          {t("files.exportPrefs")}
        </button>
        <button className="btn" onClick={() => prefsPick.current?.click()}>
          {t("files.importPrefs")}
        </button>
        <LightBurnPrefsImport notify={p.notify} onImported={backups.reload} />
      </div>
      <p className="hint">{t("files.prefsHint")}</p>
      <details className="files-details">
        <summary>{t("files.prefsBackups", { n: backups.data?.length ?? 0 })}</summary>
        {backups.error && <p className="form-error">{backups.error}</p>}
        {backups.data?.length === 0 && <p className="muted">{t("files.noPrefsBackups")}</p>}
        <div className="project-list">
          {backups.data?.map((b) => (
            <button
              key={b.file}
              className="project-row"
              onClick={() =>
                setConfirm({
                  what: t("files.confirmPrefsRestore", { when: formatModified(b.savedUnixS) }),
                  go: () =>
                    void filesApi
                      .restorePrefs(b.file)
                      .then(() => {
                        p.notify(t("files.prefsRestored"));
                        backups.reload();
                      })
                      .catch(p.report),
                })
              }
            >
              <span className="project-row-name">{formatModified(b.savedUnixS) || b.file}</span>
              <span className="project-row-meta">{formatBytes(b.size)}</span>
            </button>
          ))}
        </div>
      </details>
      <div className="section-label">{t("files.bundles")} <HelpLink to="settings#user-bundles" topic={t("files.bundles")} /></div>
      {PARTS.map((k) => (
        <CheckField key={k} label={partLabel(k)} checked={parts[k]} onChange={(v) => setParts((x) => ({ ...x, [k]: v }))} />
      ))}
      <div className="btn-row">
        <button className="btn" disabled={chosen.length === 0} onClick={() => dl(filesApi.exportBundle(chosen), "openetch-bundle.oebundle")}>
          {t("files.exportBundle")}
        </button>
        {!DESIGN_ONLY && (
          <button
            className="btn"
            disabled={!chosen.some((k) => k !== "prefs")}
            title={t("files.exportLbzipTitle")}
            onClick={() => dl(filesApi.exportBundle(chosen, "lbzip"), "openetch-bundle.lbzip")}
          >
            {t("files.exportLbzip")}
          </button>
        )}
        <button className="btn" disabled={chosen.length === 0} onClick={() => bundlePick.current?.click()}>
          {t("files.importBundle")}
        </button>
      </div>
      <p className="hint">{t("files.bundlesHint")}</p>
      {result && (
        <div className="files-result" role="status">
          <div>{t("files.bundleResult", { prefs: result.prefs ? t("files.yes") : t("files.no"), devices: result.devices.length, libraries: result.libraries.length })}</div>
          {(result.presets.length > 0 || result.images.length > 0 || result.art.length > 0) && (
            <div>
              {t("files.bundleResultLb", { presets: result.presets.length, images: result.images.length, art: result.art.join(", ") || "0" })}
            </div>
          )}
          {result.skipped.length > 0 && (
            <ul className="warn-list">
              {result.skipped.map((s) => (
                <li key={s}>{s}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {confirm && (
        <Modal
          title={t("files.confirmTitle")}
          onClose={() => setConfirm(null)}
          footer={
            <>
              <button className="btn" onClick={() => setConfirm(null)}>
                {t("common.cancel")}
              </button>
              <button
                className="btn btn-warn"
                onClick={() => {
                  const c = confirm;
                  setConfirm(null);
                  c.go();
                }}
              >
                {t("files.continue")}
              </button>
            </>
          }
        >
          <p>{confirm.what}</p>
        </Modal>
      )}
    </Modal>
  );
}

// --- notes --------------------------------------------------------------------------------------------------------

/** The Notes window: edit the project notes (one `doc.set_notes` on Save), "Show notes when the file opens". */
export function NotesWindow(p: { notes: Notes; canEdit: boolean; onSave: (n: Partial<Notes>) => Promise<unknown>; onClose: () => void }) {
  const t = useT();
  const [text, setText] = useState(p.notes.text);
  const [show, setShow] = useState(p.notes.show_on_load);
  // An edit by someone else while the window is open replaces an unchanged draft.
  const base = useRef(p.notes);
  useEffect(() => {
    if (text === base.current.text) setText(p.notes.text);
    if (show === base.current.show_on_load) setShow(p.notes.show_on_load);
    base.current = p.notes;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.notes.text, p.notes.show_on_load]);
  const changed = text !== p.notes.text || show !== p.notes.show_on_load;
  const save = () => {
    const patch: Partial<Notes> = {};
    if (text !== p.notes.text) patch.text = text;
    if (show !== p.notes.show_on_load) patch.show_on_load = show;
    void p.onSave(patch).then(() => p.onClose());
  };
  return (
    <Modal
      help="files-and-projects#project-notes"
      title={t("files.notesTitle")}
      onClose={p.onClose}
      wide
      footer={
        <>
          <button className="btn" onClick={p.onClose}>
            {changed ? t("common.cancel") : t("common.close")}
          </button>
          <button className="btn btn-primary" disabled={!changed || !p.canEdit} onClick={save}>
            {t("files.save")}
          </button>
        </>
      }
    >
      <textarea className="files-notes" value={text} autoFocus maxLength={100000} placeholder={t("files.notesPlaceholder")} onChange={(e) => setText(e.target.value)} />
      <CheckField label={t("files.notesShowOnLoad")} checked={show} onChange={setShow} />
      <p className="hint">{t("files.notesHint")}</p>
    </Modal>
  );
}

// --- thumbnails ---------------------------------------------------------------------------------------------------

const thumbCache = new Map<string, string>();

/** A saved project's thumbnail (GET /api/projects/{name}/thumbnail.png), fetched with the auth header. */
export function ProjectThumb(p: { name: string; modified?: number }) {
  const key = `${p.name}@${p.modified ?? 0}`;
  const [url, setUrl] = useState<string | null>(thumbCache.get(key) ?? null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (thumbCache.has(key)) {
      setUrl(thumbCache.get(key) ?? null);
      return;
    }
    let alive = true;
    filesApi
      .thumbnail(p.name, 128)
      .then((d) => {
        const u = URL.createObjectURL(d.blob);
        thumbCache.set(key, u);
        // Old entries are only forgotten, never revoked: a mounted thumbnail may still show one.
        if (thumbCache.size > 64) thumbCache.delete(thumbCache.keys().next().value as string);
        if (alive) setUrl(u);
      })
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [key, p.name]);
  return <span className="files-thumb" aria-hidden="true">{url && !failed ? <img src={url} alt="" /> : null}</span>;
}
