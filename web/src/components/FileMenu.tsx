// The File menu, Fusion-style at the far left of the top bar: New, Open (engine projects or a file from this device),
// Save / Save as (engine projects), Import LightBurn (.lbrn2, replace or merge), Export to LightBurn and Download
// .openetch. `useFileActions` holds the state and actions; `FileMenu` is the tap-to-open dropdown and `FileDialogs`
// the dialogs and the dismissible warnings panel. Files dropped on the window or picked with Import are routed in
// App.tsx (api/drop.ts); LightBurn files land in the import dialog here, which offers Open (replace) or Import (merge). Downloads are fetched with the Authorization header and saved from
// a blob link, so they work with token auth. Replacing a document with unsaved changes asks first.
//
// LightBurn field size: the dialog reads each file's DeviceName and shows the size the engine's rule picks (the file's
// LightBurn device, else the engine's LightBurn device, else the design's, else the machine's; api/lightburn.ts). It
// sends no `field_mm` unless the user edits the size, so the engine applies the same rule and reports `field_source`.
// With the device registry (components/Devices.tsx) the devices are OpenEtch's devices and the engine's own device is
// the active one; a gantry device is laid out for its workspace (W x H and origin), not for a square field.
//
// The dialog preselects Open (replace) when the design has no objects, Import (merge) otherwise (`defaultImportMode`).
// The top bar and download names use the opened file's name, which the engine keeps as `display_name` across reloads.

import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, execute } from "../api/client.ts";
import { createdIds } from "../api/drop.ts";
import {
  fieldSourceText,
  labelledFieldChoices,
  lbrnImportField,
  readLbrnDeviceName,
  sharedField,
  type FieldChoice,
  type ImportFieldDefault,
  type LightBurnDevice,
} from "../api/lightburn.ts";
import * as C from "../api/commands.ts";
import { isGalvo, originLabel, workspaceText, type DeviceSummary } from "../api/devices.ts";
import {
  MAX_UPLOAD_BYTES,
  commandResult,
  currentProjectName,
  defaultImportMode,
  downloadName,
  exportBaseName,
  exportFieldDefault,
  exportFieldOrigin,
  exportFieldOriginText,
  fileError,
  fileFormat,
  fileStem,
  formatBytes,
  formatModified,
  groupWarnings,
  importFieldOf,
  importMessage,
  importOutcome,
  isSaveConflict,
  needsConfirm,
  openedFileName,
  projectClash,
  projectDirty,
  projectList,
  suggestProjectName,
  validField,
  validateProjectName,
  warningList,
  type ImportMode,
} from "../api/project.ts";
import type { ProjectEntry } from "../api/types.ts";
import type { Engine } from "../api/useEngine.ts";
import { Icon, Modal, fmt } from "./ui.tsx";

type Notify = (msg: string, kind?: "info" | "error") => void;

/** How a save ended: saved, refused because another project has that name (ask to replace), or failed (reported). */
export type SaveOutcome = "saved" | "exists" | "failed";

/** Which File dialog is open. `back` is where Cancel on a confirmation returns to. */
type FileModal =
  | { kind: "open" }
  | { kind: "saveAs" }
  | { kind: "import"; files: File[]; mode: ImportMode; nonce?: number }
  | { kind: "export" }
  | { kind: "confirm"; title: string; message: string; confirmLabel: string; action: () => void; back: FileModal | null };

export interface FileActions {
  /** The project the document is saved as, or null for an unsaved document. */
  projectName: string | null;
  /** Unsaved changes; null when the engine reports no project state. */
  dirty: boolean | null;
  busy: boolean;
  /** Mark the busy state while App imports dropped files, so File actions wait. */
  setBusy: (b: boolean) => void;
  modal: FileModal | null;
  warnings: { title: string; items: string[] } | null;
  /** Name offered by Save as. */
  suggestedName: string;
  /** Field size the LightBurn import defaults to before the LightBurn device rule (the design's, else the machine's). */
  defaultFieldMm: number;
  /** LightBurn devices the engine knows (for the field-size rule and chips). */
  lightburnDevices: LightBurnDevice[];
  /** The engine's own device (the active device, or on older engines LightBurn's default), or null. */
  lightburnDevice: string | null;
  /** Whether `lightburnDevice` is the active OpenEtch device or (older engines) LightBurn's default device. */
  lightburnDeviceKind: "default-device" | "active-device";
  /** The active device (null on engines without the device registry), for the LightBurn dialogs. */
  activeDevice: DeviceSummary | null;
  /** The name of the file the design was opened from (engine `display_name`, else this page's), or null. */
  openedName: string | null;
  docFieldMm: number | null;
  machineFieldMm: number | null;
  /** Field size the document's LightBurn art was read at (opened, or the last merge the engine remembers), or null. */
  importedFieldMm: number | null;
  /** Field size Export to LightBurn starts with (see `exportFieldDefault`). */
  exportFieldMm: number;
  setModal: (m: FileModal | null) => void;
  dismissWarnings: () => void;
  newDocument: () => void;
  save: () => void;
  /** Save under `name`; `overwrite: false` lets the engine refuse to replace another project (outcome "exists"). */
  saveAs: (name: string, overwrite?: boolean) => Promise<SaveOutcome>;
  openProject: (name: string) => void;
  /**
   * Open a .openetch (replace), or put a .lbrn2 in the import dialog with `lbrn2Mode` preselected (default: Open when
   * the design is empty, else Import; see `defaultImportMode`).
   */
  uploadFile: (file: File, lbrn2Mode?: ImportMode) => void;
  /** Open the LightBurn Open/Import dialog with no file yet and the default mode. */
  openLbrn2Dialog: () => void;
  /**
   * Import LightBurn files one after another (several files are always merged). `fieldMm` undefined lets the engine
   * pick each file's field size (see `lbrnImportField`); a number is the size the user typed, used for every file.
   */
  importLbrn2: (files: File[], mode: ImportMode, fieldMm?: number) => void;
  /** Export the document as a LightBurn project laid out for `fieldMm` (undefined: the active device's workspace). */
  exportLbrn2: (fieldMm?: number) => void;
  downloadOpenetch: () => void;
}

/** Save a blob through a temporary download link. */
function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Safari starts the download asynchronously; keep the URL alive for a while.
  window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

export function useFileActions(p: {
  engine: Engine;
  machineFieldMm: number | null;
  fallbackFieldMm: number;
  notify: Notify;
  report: (e: unknown) => void;
  /** Called with the ids of objects an import created, so they can be selected (LightBurn "Select imported shapes"). */
  onImported?: (ids: string[]) => void;
  /**
   * The devices the LightBurn dialogs match files against and the engine's own device (absent on engines without
   * LightBurn support): the device registry and its active device, or on older engines LightBurn's devices.
   */
  lightburn?: {
    devices: LightBurnDevice[];
    engineDevice: string | null;
    engineKind?: "default-device" | "active-device";
    active?: DeviceSummary | null;
  };
}): FileActions {
  const { engine, notify, report } = p;
  const onImportedRef = useRef(p.onImported);
  onImportedRef.current = p.onImported;
  const { doc, project } = engine;
  const [busy, setBusy] = useState(false);
  const [modal, setModal] = useState<FileModal | null>(null);
  const [warnings, setWarnings] = useState<{ title: string; items: string[] } | null>(null);
  const [suggestedName, setSuggestedName] = useState("");
  // The engine's project name when this UI last replaced the document (see `currentProjectName`); undefined when the
  // document is the project the engine reports.
  const [detachedFrom, setDetachedFrom] = useState<string | null | undefined>(undefined);

  // A file opened from this device has never been saved on the engine, which reports it as clean; it counts as
  // unsaved until it is saved (or replaced by New or a saved project).
  const [loadedUnsaved, setLoadedUnsaved] = useState(false);
  // LightBurn files do not record their field size: remember the one a LightBurn file was opened at, so Export offers
  // it and an open/export round trip does not shift the artwork (cleared when another document replaces it).
  const [importedFieldMm, setImportedFieldMm] = useState<number | null>(null);
  // Name of the file the design was opened from (Open replaces it), for exports while there is no saved project.
  const [openedName, setOpenedName] = useState<string | null>(null);
  const projectName = currentProjectName(project, detachedFrom);
  const engineDirty = projectDirty(project, doc?.revision ?? null);
  const dirty = loadedUnsaved && engineDirty !== null && projectName === null ? true : engineDirty;
  const docFieldMm = validField(doc?.field_mm);
  const defaultFieldMm = docFieldMm ?? p.machineFieldMm ?? p.fallbackFieldMm;
  // As the engine's own default: a merged file's size (remembered in the document), else the one a file was opened at.
  const exportFromFieldMm = importFieldOf(doc, importedFieldMm);
  const exportFieldMm = exportFieldDefault(exportFromFieldMm, docFieldMm, p.machineFieldMm, p.fallbackFieldMm);

  // Once the engine reports another project (opened or saved elsewhere, or cleared), the document follows it again.
  useEffect(() => {
    if (detachedFrom !== undefined && (project?.name ?? null) !== detachedFrom) setDetachedFrom(undefined);
  }, [project?.name, detachedFrom]);

  // The opened file's name as the engine keeps it (survives a reload), else this page's own record.
  const openedFile = openedFileName(project, openedName);

  // Latest values for callbacks that run after a confirmation.
  const live = useRef({ project, doc, dirty, projectName, openedName: openedFile, active: p.lightburn?.active ?? null });
  live.current = { project, doc, dirty, projectName, openedName: openedFile, active: p.lightburn?.active ?? null };

  const task = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
      } catch (e) {
        report(e);
      } finally {
        setBusy(false);
      }
    },
    [report],
  );

  /** Run `action` now, or after a confirmation when the document has unsaved changes. */
  const guard = useCallback((what: string, confirmLabel: string, action: () => void, back: FileModal | null) => {
    const { dirty: d, doc: dd, projectName: n } = live.current;
    if (!needsConfirm(d, dd)) {
      action();
      return;
    }
    const subject = n ? `"${n}" has unsaved changes.` : "This document has not been saved.";
    setModal({
      kind: "confirm",
      title: "Unsaved changes",
      message: `${subject} ${what} replaces it. You can still get it back with Undo.`,
      confirmLabel,
      action,
      back,
    });
  }, []);

  /** After this UI replaced the document: Save must ask for a name, offering `suggest`. */
  const detach = useCallback((suggest: string) => {
    setDetachedFrom(live.current.project?.name ?? null);
    setSuggestedName(suggest);
  }, []);

  const showWarnings = useCallback((title: string, items: string[]) => {
    setWarnings(items.length ? { title, items } : null);
  }, []);

  const newDocument = useCallback(() => {
    guard("A new document", "Discard and start new", () => {
      setModal(null);
      void task(async () => {
        await engine.run(C.docNew());
        detach("");
        setLoadedUnsaved(false);
        setImportedFieldMm(null);
        setOpenedName(null);
        notify("New document.");
      });
    }, null);
  }, [guard, task, engine, detach, notify]);

  const saveAs = useCallback(
    async (name: string, overwrite?: boolean): Promise<SaveOutcome> => {
      const problem = validateProjectName(name);
      if (problem) {
        notify(problem, "error");
        return "failed";
      }
      setBusy(true);
      try {
        // Keep the document's own name (Browser tree, exports, the saved file) in step with the project name.
        const d = live.current.doc;
        if (d && d.name !== name && engine.has(C.OPT.docRename)) await engine.run(C.docRename(name)).catch(() => undefined);
        await api.saveProject(name, overwrite);
        setDetachedFrom(undefined);
        setSuggestedName("");
        setLoadedUnsaved(false);
        setOpenedName(null);
        notify(`Saved "${name}".`);
        return "saved";
      } catch (e) {
        if (overwrite === false && isSaveConflict(e)) return "exists";
        report(e);
        return "failed";
      } finally {
        setBusy(false);
        void engine.refreshProject();
      }
    },
    [engine, notify, report],
  );

  const save = useCallback(() => {
    const name = live.current.projectName;
    if (name === null) setModal({ kind: "saveAs" });
    else void saveAs(name);
  }, [saveAs]);

  const openProject = useCallback(
    (name: string) => {
      guard(`Opening "${name}"`, "Discard and open", () => {
        setModal(null);
        void task(async () => {
          await api.openProject(name);
          setDetachedFrom(undefined);
          setSuggestedName("");
          setLoadedUnsaved(false);
          setImportedFieldMm(null);
          setOpenedName(null);
          notify(`Opened "${name}".`);
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, notify],
  );

  const openLbrn2Dialog = useCallback(() => setModal({ kind: "import", files: [], mode: defaultImportMode(live.current.doc) }), []);

  const uploadFile = useCallback(
    (file: File, lbrn2Mode?: ImportMode) => {
      const format = fileFormat(file.name);
      if (format === null) {
        notify("Choose a .openetch project or a LightBurn .lbrn2 file.", "error");
        return;
      }
      if (format === "lbrn2") {
        // LightBurn files need a field size and a replace/merge choice.
        setModal({ kind: "import", files: [file], mode: lbrn2Mode ?? defaultImportMode(live.current.doc) });
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        notify(`${file.name} is larger than ${formatBytes(MAX_UPLOAD_BYTES)}.`, "error");
        return;
      }
      guard(`Opening ${file.name}`, "Discard and open", () => {
        setModal(null);
        void task(async () => {
          const result = commandResult(await api.upload(file, "openetch", "replace", undefined, { name: file.name }).catch((e: unknown) => Promise.reject(fileError(e, file.name))));
          detach(suggestProjectName(fileStem(file.name)));
          setLoadedUnsaved(true);
          setImportedFieldMm(null);
          setOpenedName(fileStem(file.name));
          notify(`Opened ${file.name}. Save it to keep it on the engine.`);
          showWarnings(`Opening ${file.name}`, warningList(result));
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, detach, notify, showWarnings],
  );

  const importLbrn2 = useCallback(
    (files: File[], requested: ImportMode, fieldMm?: number) => {
      // Several files at once are always merged: replacing with each in turn would keep only the last.
      const mode: ImportMode = files.length > 1 ? "merge" : requested;
      const go = () => {
        setModal(null);
        void task(async () => {
          const created: string[] = [];
          const warnings: string[] = [];
          const failed: string[] = [];
          let objects = 0;
          let last: ReturnType<typeof importOutcome> | null = null;
          for (const file of files) {
            try {
              // The upload route takes the raw file (up to 50 MB) and the field size it was laid out for.
              if (file.size > MAX_UPLOAD_BYTES) throw new Error(`${file.name} is larger than ${formatBytes(MAX_UPLOAD_BYTES)}.`);
              // No size: the engine picks it (the file's LightBurn device first) and reports it with `field_source`.
              const result = commandResult(await api.upload(file, "lbrn2", mode, fieldMm, { name: fileStem(file.name) }).catch((e: unknown) => Promise.reject(fileError(e, file.name))));
              const outcome = importOutcome(result);
              last = outcome;
              objects += outcome.objects;
              created.push(...createdIds(result));
              warnings.push(...outcome.warnings.map((w) => (files.length > 1 ? `${file.name}: ${w}` : w)));
            } catch (e) {
              if (files.length === 1) throw e;
              failed.push(`${file.name}: ${e instanceof Error ? e.message : String(e)}`);
            }
          }
          await engine.refreshDoc();
          if (mode === "replace") {
            detach(suggestProjectName(fileStem(files[0].name)));
            setLoadedUnsaved(true);
            setImportedFieldMm(last?.fieldMm ?? fieldMm ?? validField(live.current.doc?.field_mm));
            setOpenedName(fileStem(files[0].name));
          }
          if (created.length) onImportedRef.current?.(created);
          if (files.length === 1 && last) notify(importMessage(last, mode));
          else
            notify(
              `Merged ${files.length - failed.length} of ${files.length} LightBurn files (${objects} object${objects === 1 ? "" : "s"}).`,
              failed.length ? "error" : "info",
            );
          for (const f of failed) notify(f, "error");
          showWarnings(files.length > 1 ? `Import of ${files.length} LightBurn files` : `Import of ${files[0].name}`, warnings);
        });
      };
      if (mode === "replace") guard(`Importing ${files[0].name}`, "Discard and import", go, { kind: "import", files, mode });
      else go();
    },
    [guard, task, engine, detach, notify, showWarnings],
  );

  const baseName = () => exportBaseName(live.current.projectName, live.current.openedName, live.current.doc?.name);

  const exportLbrn2 = useCallback((fieldMm?: number) => {
    setModal(null);
    void task(async () => {
      // Always the size the Export dialog showed, so the file is laid out exactly as the user saw it; no size for a
      // gantry device, whose workspace and origin the engine applies.
      const res = await execute(C.exportLbrn2(fieldMm));
      const r = res.result as { content?: unknown } | null;
      if (!r || typeof r.content !== "string") throw new Error("the engine returned no LightBurn content");
      const name = downloadName(baseName(), "lbrn2");
      saveBlob(new Blob([r.content], { type: "application/xml" }), name);
      const w = warningList(r);
      const dev = live.current.active;
      const what = fieldMm !== undefined ? `a ${fmt(fieldMm, 3)} mm field` : dev ? `${dev.display_name} (${workspaceText(dev.workspace_mm)})` : "the active device";
      notify(`Exported ${name} for ${what}.${w.length ? ` ${w.length} warning${w.length === 1 ? "" : "s"}.` : ""}`);
      showWarnings(`Export of ${name}`, w);
    });
  }, [task, notify, showWarnings]);

  const downloadOpenetch = useCallback(() => {
    void task(async () => {
      const { blob, filename } = await api.download("openetch").catch((e: unknown) => Promise.reject(fileError(e, "Download")));
      // The engine names the file after its project, which this UI may have replaced (see `currentProjectName`).
      const name = (live.current.projectName !== null ? filename : null) ?? downloadName(baseName(), "openetch");
      saveBlob(blob, name);
      notify(`Downloaded ${name}.`);
    });
  }, [task, notify]);

  return {
    projectName,
    dirty,
    busy,
    setBusy,
    modal,
    warnings,
    suggestedName,
    defaultFieldMm,
    lightburnDevices: p.lightburn?.devices ?? [],
    lightburnDevice: p.lightburn?.engineDevice ?? null,
    lightburnDeviceKind: p.lightburn?.engineKind ?? "default-device",
    activeDevice: p.lightburn?.active ?? null,
    openedName: openedFile,
    docFieldMm,
    machineFieldMm: p.machineFieldMm,
    importedFieldMm: exportFromFieldMm,
    exportFieldMm,
    setModal,
    dismissWarnings: () => setWarnings(null),
    newDocument,
    save,
    saveAs,
    openProject,
    uploadFile,
    openLbrn2Dialog,
    importLbrn2,
    exportLbrn2,
    downloadOpenetch,
  };
}

/** "Ctrl+S", or "⌘S" on Apple devices. */
export function saveShortcut(): string {
  const plat = typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent;
  return /Mac|iPhone|iPad|iPod/.test(plat) ? "⌘S" : "Ctrl+S";
}

/** "Ctrl+X" or "⌘X" for a menu hint. */
function shortcut(key: string, shift = false): string {
  const mac = saveShortcut().startsWith("⌘");
  return mac ? `${shift ? "⇧" : ""}⌘${key}` : `Ctrl+${shift ? "Shift+" : ""}${key}`;
}

/**
 * The File button and its tap-to-open dropdown. `onImport` opens the Import picker (also Ctrl+I or a drop); `onDevices`
 * opens the Devices window (absent on engines without the device registry).
 */
export function FileMenu({ file, onImport, onDevices }: { file: FileActions; onImport: () => void; onDevices?: () => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const item = (label: string, action: () => void, hint?: string) => (
    <button
      role="menuitem"
      className="menu-item"
      disabled={file.busy}
      onClick={() => {
        setOpen(false);
        action();
      }}
    >
      <span>{label}</span>
      {hint && <span className="menu-hint">{hint}</span>}
    </button>
  );
  return (
    <div className="file-menu" ref={root}>
      <button className={`file-btn${open ? " open" : ""}`} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        File
        <Icon name="chevronDown" size={16} />
      </button>
      {open && (
        <div className="menu" role="menu" aria-label="File">
          {item("New", file.newDocument)}
          {item("Open…", () => file.setModal({ kind: "open" }), shortcut("O"))}
          {item("Save", file.save, saveShortcut())}
          {item("Save as…", () => file.setModal({ kind: "saveAs" }), shortcut("S", true))}
          <div className="menu-sep" role="separator" />
          {item("Import…", onImport, shortcut("I"))}
          {item("Import LightBurn…", file.openLbrn2Dialog, ".lbrn2")}
          {item("Export to LightBurn…", () => file.setModal({ kind: "export" }), ".lbrn2")}
          {onDevices && (
            <>
              <div className="menu-sep" role="separator" />
              {item("Devices…", onDevices)}
            </>
          )}
          <div className="menu-sep" role="separator" />
          {item("Download .openetch", file.downloadOpenetch)}
        </div>
      )}
    </div>
  );
}

/** Project name and unsaved-changes dot for the top bar. */
export function ProjectIndicator({ name, docName, dirty }: { name: string | null; docName: string | null; dirty: boolean | null }) {
  const label = name ?? docName;
  const title = [name ? `Project "${name}"` : "Not saved as a project yet", dirty ? "Unsaved changes" : dirty === false ? "All changes saved" : ""]
    .filter(Boolean)
    .join("\n");
  if (!label && !dirty) return null;
  return (
    <span className="project" title={title}>
      {label && <span className={`doc-name${name ? "" : " unsaved"}`}>{label}</span>}
      {dirty && <span className="dirty-dot" role="img" aria-label="Unsaved changes" />}
    </span>
  );
}

/** Every File dialog plus the warnings panel, rendered into <body> so they do not inherit the top bar's styles. */
export function FileDialogs({ file, docName }: { file: FileActions; docName: string | null }) {
  const { setModal } = file;
  const close = useCallback(() => setModal(null), [setModal]);
  const m = file.modal;
  return createPortal(
    <>
      {m?.kind === "open" && <OpenDialog file={file} onClose={close} />}
      {m?.kind === "saveAs" && <SaveAsDialog file={file} docName={docName} onClose={close} />}
      {m?.kind === "import" && <ImportDialog key={m.nonce ?? 0} file={file} initialFiles={m.files} initialMode={m.mode} onClose={close} />}
      {m?.kind === "export" && <ExportDialog file={file} onClose={close} />}
      {m?.kind === "confirm" && (
        <Modal
          title={m.title}
          onClose={() => file.setModal(m.back)}
          footer={
            <>
              <button className="btn" onClick={() => file.setModal(m.back)}>
                Cancel
              </button>
              <button className="btn btn-warn" onClick={m.action}>
                {m.confirmLabel}
              </button>
            </>
          }
        >
          <p>{m.message}</p>
        </Modal>
      )}
      {file.warnings && <WarningsPanel title={file.warnings.title} items={file.warnings.items} onClose={file.dismissWarnings} />}
    </>,
    document.body,
  );
}

function WarningsPanel(p: { title: string; items: string[]; onClose: () => void }) {
  const rows = groupWarnings(p.items);
  return (
    <div className="warn-panel" role="status" aria-live="polite">
      <div className="warn-panel-head">
        <span>
          {p.title}: {p.items.length} warning{p.items.length === 1 ? "" : "s"}
        </span>
        <button className="icon-btn small" onClick={p.onClose} aria-label="Dismiss warnings">
          <Icon name="close" />
        </button>
      </div>
      <ul className="warn-list">
        {rows.map((r) => (
          <li key={r.text}>
            {r.text}
            {r.count > 1 && <span className="warn-count">×{r.count}</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Load the engine's project list once; `null` while loading. */
function useProjectList(): { list: ProjectEntry[] | null; error: string | null } {
  const [list, setList] = useState<ProjectEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api
      .projects()
      .then((r) => alive && setList(projectList(r)))
      .catch((e) => {
        if (!alive) return;
        setList([]);
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, []);
  return { list, error };
}

function OpenDialog({ file, onClose }: { file: FileActions; onClose: () => void }) {
  const { list, error } = useProjectList();
  const input = useRef<HTMLInputElement>(null);
  return (
    <Modal
      title="Open"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={() => input.current?.click()} disabled={file.busy}>
            <Icon name="upload" /> Upload from this device
          </button>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
        </>
      }
    >
      <input
        ref={input}
        type="file"
        accept=".openetch,.lbrn2"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          // The user chose Open: a LightBurn file replaces the design.
          if (f) file.uploadFile(f, "replace");
        }}
      />
      <div className="section-label">Projects on the engine</div>
      {list === null && <p className="muted">Loading…</p>}
      {error && <p className="form-error">{error}</p>}
      {list !== null && !error && list.length === 0 && <p className="muted">No saved projects yet. Use Save to keep one here.</p>}
      {list !== null && list.length > 0 && (
        <div className="project-list">
          {list.map((pr) => (
            <button
              key={pr.name}
              className={`project-row${pr.name === file.projectName ? " current" : ""}`}
              disabled={file.busy}
              onClick={() => file.openProject(pr.name)}
            >
              <span className="project-row-name">{pr.name}</span>
              <span className="project-row-meta">
                {[formatModified(pr.modified_unix_s), formatBytes(pr.size)].filter(Boolean).join(" · ")}
              </span>
            </button>
          ))}
        </div>
      )}
      <p className="hint">Upload opens a .openetch project, or imports a LightBurn .lbrn2 file.</p>
    </Modal>
  );
}

function SaveAsDialog({ file, docName, onClose }: { file: FileActions; docName: string | null; onClose: () => void }) {
  const initial = file.suggestedName || file.projectName || (docName && docName !== "Untitled" ? suggestProjectName(docName) : "");
  const [name, setName] = useState(initial);
  const [confirmReplace, setConfirmReplace] = useState(false);
  // The engine refused the save because a project of that name exists that the list did not show (case-only clash,
  // or saved meanwhile from elsewhere).
  const [refused, setRefused] = useState<string | null>(null);
  const { list } = useProjectList();
  const trimmed = name.trim();
  const problem = trimmed ? validateProjectName(trimmed) : null;
  // Case-insensitive: Windows and macOS would replace "ok name" when saving "OK NAME".
  const existing = projectClash(list, trimmed, file.projectName) ?? (refused === trimmed ? trimmed : null);
  const submit = async () => {
    if (!trimmed || problem || file.busy) return;
    if (existing !== null && !confirmReplace) {
      setConfirmReplace(true);
      return;
    }
    // Only an explicit Replace may overwrite; otherwise the engine refuses with 409 and this dialog asks.
    const outcome = await file.saveAs(trimmed, confirmReplace);
    if (outcome === "saved") onClose();
    else if (outcome === "exists") {
      setRefused(trimmed);
      setConfirmReplace(true);
    }
  };
  return (
    <Modal
      title="Save as"
      onClose={onClose}
      footer={
        <>
          {problem && <span className="form-error">{problem}</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className={`btn ${confirmReplace ? "btn-warn" : "btn-primary"}`} disabled={!trimmed || !!problem || file.busy} onClick={() => void submit()}>
            {confirmReplace ? "Replace" : "Save"}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">Project name</span>
        <span className="field-input">
          <input
            type="text"
            autoFocus
            value={name}
            maxLength={200}
            placeholder="e.g. Namecards (brass)"
            onChange={(e) => {
              setName(e.target.value);
              setConfirmReplace(false);
            }}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
        </span>
      </label>
      {existing !== null && <p className="hint warn-text">A project named "{existing}" exists. Saving replaces it.</p>}
      <p className="hint">Saved on the engine as {trimmed && !problem ? `"${trimmed}.openetch"` : "<name>.openetch"}.</p>
    </Modal>
  );
}

/** The typed field size, or null when it is not a usable size. */
function parseField(text: string): number | null {
  const n = Number(text.trim());
  return text.trim() !== "" && Number.isFinite(n) && n > 0 && n <= 2000 ? n : null;
}

/** Field size text box plus one-tap sizes, shown once the user chooses to edit the size. */
function FieldEditor(p: { value: string; onChange: (v: string) => void; onEnter: () => void; choices: FieldChoice[]; autoFocus?: boolean }) {
  const n = parseField(p.value);
  return (
    <>
      <label className="field">
        <span className="field-label">Field size</span>
        <span className="field-input">
          <input
            type="text"
            inputMode="decimal"
            autoFocus={p.autoFocus}
            value={p.value}
            onChange={(e) => p.onChange(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && p.onEnter()}
          />
          <span className="unit">mm</span>
        </span>
      </label>
      <div className="chip-row">
        {p.choices.map((c) => (
          <button key={c.mm} className={`chip${n !== null && Math.abs(c.mm - n) < 1e-3 ? " active" : ""}`} onClick={() => p.onChange(fmt(c.mm, 3))}>
            {fmt(c.mm, 3)} mm
            {c.label ? ` (${c.label})` : ""}
          </button>
        ))}
      </div>
    </>
  );
}

/**
 * "Field size 153.8 mm — from your LightBurn device JCZFiber  Edit", or for a gantry device "Workspace 600 × 400 mm —
 * from your active device Shop CO2" (no Edit: the device's workspace and origin are used as they are).
 */
function FieldSummary(p: { mm: number; source: string; onEdit?: () => void; workspaceMm?: [number, number] | null; machineMm?: number | null }) {
  // LightBurn stores the field to 0.1 mm (153.8 for a 153.75 mm lens): say so instead of showing two unexplained numbers.
  const rounded = p.machineMm != null && !p.workspaceMm && sameField(p.mm, p.machineMm) && Math.abs(p.mm - p.machineMm) >= 1e-3;
  return (
    <p className="field-summary">
      <span>
        {p.workspaceMm ? (
          <>
            Workspace <strong>{workspaceText(p.workspaceMm)}</strong>
          </>
        ) : (
          <>
            Field size <strong>{fmt(p.mm, 3)} mm</strong>
          </>
        )}{" "}
        — {p.source}
        {rounded && <span className="muted"> (LightBurn's rounding of this laser's {fmt(p.machineMm as number, 3)} mm field)</span>}
      </span>
      {p.onEdit && !p.workspaceMm && (
        <button className="link-btn" onClick={p.onEdit}>
          Edit
        </button>
      )}
    </p>
  );
}

/** Two field sizes that are the same lens: LightBurn keeps 0.1 mm, EzCad and the .cor file keep more. */
function sameField(a: number, b: number): boolean {
  return Math.abs(a - b) <= 0.05 + 1e-9;
}

/** One-tap sizes: every LightBurn device's, the design's, the machine's, 110 and 150. */
function fieldChoiceList(file: FileActions, first: { mm: number | null; label: string | null }[] = []): FieldChoice[] {
  return labelledFieldChoices([
    ...first,
    // Galvo fields only: a gantry workspace is not a field size.
    ...file.lightburnDevices.filter((d) => d.kind === undefined || isGalvo(d.kind)).map((d) => ({ mm: d.field_mm, label: d.name })),
    { mm: file.docFieldMm, label: "current design" },
    { mm: file.machineFieldMm, label: "this laser" },
    { mm: 110, label: null },
    { mm: 150, label: null },
  ]);
}

/** Each picked file's DeviceName (null when it has none), or null while they are being read. */
function useDeviceNames(files: File[]): (string | null)[] | null {
  const [names, setNames] = useState<{ files: File[]; names: (string | null)[] } | null>(null);
  useEffect(() => {
    let alive = true;
    void Promise.all(files.map((f) => readLbrnDeviceName(f))).then((n) => alive && setNames({ files, names: n }));
    return () => {
      alive = false;
    };
  }, [files]);
  return names && names.files === files ? names.names : null;
}

function ImportDialog(p: { file: FileActions; initialFiles: File[]; initialMode: ImportMode; onClose: () => void }) {
  const { file } = p;
  const [picked, setPicked] = useState<File[]>(p.initialFiles);
  const [mode, setMode] = useState<ImportMode>(p.initialMode);
  // null: the engine picks the size (shown below); text: the size the user is typing.
  const [field, setField] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const names = useDeviceNames(picked);
  const several = picked.length > 1;
  const effective: ImportMode = several ? "merge" : mode;
  const defaults: ImportFieldDefault[] = (picked.length ? picked : [null]).map((_, i) =>
    lbrnImportField({
      fileDevice: names?.[i] ?? null,
      devices: file.lightburnDevices,
      engineDevice: file.lightburnDevice,
      engineKind: file.lightburnDeviceKind,
      docField: file.docFieldMm,
      machineField: file.machineFieldMm,
      fallback: file.defaultFieldMm,
    }),
  );
  const shared = sharedField(defaults);
  const unknown = [...new Set(defaults.map((d) => d.unknownFileDevice).filter((d): d is string => d !== null))];
  const typed = field === null ? null : parseField(field);
  const reading = picked.length > 0 && names === null;
  const error = field !== null && typed === null ? "Field size must be a number between 0 and 2000 mm." : null;
  const ready = picked.length > 0 && !reading && error === null && !file.busy;
  const submit = () => {
    if (ready) file.importLbrn2(picked, effective, typed ?? undefined);
  };
  const edit = () => setField(fmt((shared ?? defaults[0]).fieldMm, 3));
  return (
    <Modal
      title="Open or import LightBurn project"
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!ready} onClick={submit}>
            {effective === "replace" ? "Open" : "Import"}
          </button>
        </>
      }
    >
      <input
        ref={input}
        type="file"
        accept=".lbrn2"
        multiple
        hidden
        onChange={(e) => {
          const f = Array.from(e.target.files ?? []);
          e.target.value = "";
          if (f.length) setPicked(f);
        }}
      />
      <div className="field">
        <span className="field-label">{several ? "Files" : "File"}</span>
        <button className="btn file-pick" onClick={() => input.current?.click()}>
          <Icon name="upload" />
          <span className="file-pick-name">
            {picked.length === 0
              ? "Choose or drop a .lbrn2 file…"
              : several
                ? `${picked.length} files (${formatBytes(picked.reduce((a, f) => a + f.size, 0))})`
                : `${picked[0].name} (${formatBytes(picked[0].size)})`}
          </span>
        </button>
      </div>
      {several && <p className="hint">{picked.map((f) => f.name).join(", ")}</p>}
      <div className="segmented" role="radiogroup" aria-label="Open or import">
        <button
          role="radio"
          aria-checked={effective === "replace"}
          className={effective === "replace" ? "active" : ""}
          disabled={several}
          title={several ? "Several files are always imported into your design" : "Replace your current design with this file"}
          onClick={() => setMode("replace")}
        >
          Open (replace)
        </button>
        <button
          role="radio"
          aria-checked={effective === "merge"}
          className={effective === "merge" ? "active" : ""}
          title="Add this file's shapes and layers to your current design"
          onClick={() => setMode("merge")}
        >
          Import (merge)
        </button>
      </div>
      <p className="hint">
        Open replaces your current design. Import adds this file's shapes and layers to it.
        {several ? " Several files are always imported." : ""}
      </p>
      {field === null ? (
        reading ? (
          <p className="field-summary muted">Reading the file…</p>
        ) : shared ? (
          <FieldSummary mm={shared.fieldMm} source={fieldSourceText(shared)} onEdit={edit} workspaceMm={shared.workspaceMm} machineMm={p.file.machineFieldMm} />
        ) : (
          <>
            <p className="field-summary">
              <span>Field size: each file uses its own</span>
              <button className="link-btn" onClick={edit}>
                Edit
              </button>
            </p>
            <ul className="field-per-file">
              {picked.map((f, i) => (
                <li key={`${f.name}-${i}`}>
                  {f.name}: {defaults[i].workspaceMm ? workspaceText(defaults[i].workspaceMm ?? [0, 0]) : `${fmt(defaults[i].fieldMm, 3)} mm`},{" "}
                  {fieldSourceText(defaults[i])}
                </li>
              ))}
            </ul>
          </>
        )
      ) : (
        <>
          <FieldEditor
            value={field}
            onChange={setField}
            onEnter={submit}
            autoFocus
            choices={fieldChoiceList(file, defaults.map((d) => ({ mm: d.fieldMm, label: d.device })))}
          />
          <p className="hint">
            LightBurn files don't store the field size. Use the Field Size from LightBurn's Device Settings when this file
            was made.
          </p>
          <button className="link-btn" onClick={() => setField(null)}>
            Use the automatic size
          </button>
        </>
      )}
      {field === null && !reading && unknown.length > 0 && (
        <p className="hint">
          {unknown.length === 1 ? `This file was made for the LightBurn device "${unknown[0]}"` : `These files were made for LightBurn devices ${unknown.map((u) => `"${u}"`).join(", ")}`}
          {file.lightburnDeviceKind === "active-device"
            ? ", which is not one of your devices. To use that device's size, import it in File > Devices."
            : ", which the engine doesn't know. To use that device's field size, load your LightBurn settings in the Laser workspace (Machine card)."}
        </p>
      )}
    </Modal>
  );
}

/** Export to LightBurn: shows the field size the project will be laid out for, and lets it be changed. */
function ExportDialog({ file, onClose }: { file: FileActions; onClose: () => void }) {
  const [field, setField] = useState<string | null>(null);
  const typed = field === null ? null : parseField(field);
  const error = field !== null && typed === null ? "Field size must be a number between 0 and 2000 mm." : null;
  const fieldMm = field === null ? file.exportFieldMm : typed;
  const origin = exportFieldOrigin(file.importedFieldMm, file.docFieldMm, file.machineFieldMm);
  const device = file.lightburnDevices.find((d) => d.name === file.lightburnDevice) ?? (file.lightburnDevices.length === 1 ? file.lightburnDevices[0] : null);
  const deviceDiffers = device !== null && device.field_mm !== null && !sameField(device.field_mm, file.exportFieldMm);
  // A gantry device: the engine lays the file out for its workspace and origin, so there is no field size to pick.
  const active = file.activeDevice;
  const gantry = active !== null && !isGalvo(active.kind);
  const submit = () => {
    if (file.busy) return;
    if (gantry) file.exportLbrn2(undefined);
    else if (fieldMm !== null) file.exportLbrn2(fieldMm);
  };
  return (
    <Modal
      title="Export to LightBurn"
      onClose={onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" autoFocus={field === null} disabled={(!gantry && fieldMm === null) || file.busy} onClick={submit}>
            Export
          </button>
        </>
      }
    >
      {gantry && active ? (
        <>
          <FieldSummary mm={active.workspace_mm[0]} source={`from your active device ${active.display_name}`} workspaceMm={active.workspace_mm} />
          <p className="hint">
            Laid out for {active.display_name} ({active.name}, origin {originLabel(active.origin).toLowerCase()}), as LightBurn
            places it on that device. To export for another device, make it active in File &gt; Devices.
          </p>
        </>
      ) : field === null ? (
        <>
          <FieldSummary mm={file.exportFieldMm} source={exportFieldOriginText(origin)} onEdit={() => setField(fmt(file.exportFieldMm, 3))} machineMm={file.machineFieldMm} />
          {active && !sameField(active.workspace_mm[0], file.exportFieldMm) && (
            <p className="hint">Active device: {active.display_name} ({workspaceText([active.workspace_mm[0], active.workspace_mm[0]])} field).</p>
          )}
          {deviceDiffers && device && (
            <p className="hint">
              Your {active ? "active" : "LightBurn"} device {device.name} uses {fmt(device.field_mm, 3)} mm. If the file will be opened with that device,
              edit the size.
            </p>
          )}
        </>
      ) : (
        <>
          <FieldEditor
            value={field}
            onChange={setField}
            onEnter={submit}
            autoFocus
            choices={fieldChoiceList(file, [{ mm: file.importedFieldMm, label: "opened with" }])}
          />
          <p className="hint">
            LightBurn files don't store the field size. Use the Field Size from LightBurn's Device Settings on the computer
            that will open this file.
          </p>
          <button className="link-btn" onClick={() => setField(null)}>
            Use the automatic size
          </button>
        </>
      )}
    </Modal>
  );
}
