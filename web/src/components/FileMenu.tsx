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

import { useHudFixed } from "../hud/CanvasHud.tsx";
import { Fragment, isValidElement, useCallback, useEffect, useMemo, useRef, useState, type ReactElement, type ReactNode } from "react";
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
import { carryGuides, clearGuides, guideScope, UNSAVED_SCOPE } from "../guides.ts";
import { ENGINE_ACCEPT } from "../files/fileFormats.ts";
import { pickerAccept, splitPicked } from "../touch/filePick.ts";
import { ProjectThumb } from "../files/ProjectDialogs.tsx";
import { FolderTree, useProjectFolders } from "../files/ProjectFolders.tsx";
import { baseNameOf, folderOf, joinProject, projectsIn } from "../files/folders.ts";
import { Icon, Modal, fmt } from "./ui.tsx";
import { MenuButton, type MenuItem } from "./Menu.tsx";
import { FileMenuPart } from "./FileMenuPart.tsx";
import { buildFileMenu, type FileSection } from "./fileMenuModel.ts";
import { formatChord } from "./formatChord.ts";
import { usePublish } from "./Ribbon.tsx";
import { WorkspaceFileItems, workspaceFileMenuItems, type WsDialog } from "../workspace/useWorkspace.tsx";
import { useT } from "../workspace/usePrefs.ts";
import type { T } from "../workspace/i18n.ts";
import { errorText } from "../api/errorText.ts";

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
  /** Open a file that is no project (PDF, AI, a 3D model, ...) as a new project (src/files/useFilesUi.tsx). */
  openOther?: (file: File) => void;
}): FileActions {
  const { engine, notify, report } = p;
  const openOtherRef = useRef(p.openOther);
  openOtherRef.current = p.openOther;
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
  const t = useT();
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
    const subject = n ? t("fm.unsavedNamed", { name: n }) : t("fm.notSaved");
    setModal({
      kind: "confirm",
      title: t("fm.unsavedTitle"),
      message: t("fm.replaces", { subject, what }),
      confirmLabel,
      action,
      back,
    });
  }, [t]);

  /** After this UI replaced the document: Save must ask for a name, offering `suggest`. */
  const detach = useCallback((suggest: string) => {
    setDetachedFrom(live.current.project?.name ?? null);
    setSuggestedName(suggest);
  }, []);

  const showWarnings = useCallback((title: string, items: string[]) => {
    setWarnings(items.length ? { title, items } : null);
  }, []);

  const newDocument = useCallback(() => {
    guard(t("fm.aNewDocument"), t("fm.discardNew"), () => {
      setModal(null);
      void task(async () => {
        await engine.run(C.docNew());
        // The new document starts without guides; the discarded unsaved one's go with it.
        clearGuides(UNSAVED_SCOPE);
        detach("");
        setLoadedUnsaved(false);
        setImportedFieldMm(null);
        setOpenedName(null);
        notify(t("fm.newDone"));
      });
    }, null);
  }, [guard, task, engine, detach, notify, t]);

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
        const fromScope = guideScope(live.current.projectName, live.current.openedName);
        await api.saveProject(name, overwrite);
        // The guides follow the document into the project it is now saved as.
        carryGuides(fromScope, guideScope(name, null));
        setDetachedFrom(undefined);
        setSuggestedName("");
        setLoadedUnsaved(false);
        setOpenedName(null);
        notify(t("fm.saved", { name }));
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
    [engine, notify, report, t],
  );

  const save = useCallback(() => {
    const name = live.current.projectName;
    if (name === null) setModal({ kind: "saveAs" });
    else void saveAs(name);
  }, [saveAs]);

  const openProject = useCallback(
    (name: string) => {
      guard(t("fm.openingNamed", { name }), t("fm.discardOpen"), () => {
        setModal(null);
        void task(async () => {
          await api.openProject(name);
          setDetachedFrom(undefined);
          setSuggestedName("");
          setLoadedUnsaved(false);
          setImportedFieldMm(null);
          setOpenedName(null);
          notify(t("fm.opened", { name }));
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, notify, t],
  );

  const openLbrn2Dialog = useCallback(() => setModal({ kind: "import", files: [], mode: defaultImportMode(live.current.doc) }), []);

  const uploadFile = useCallback(
    (file: File, lbrn2Mode?: ImportMode) => {
      const format = fileFormat(file.name);
      if (format === null) {
        if (openOtherRef.current) {
          setModal(null);
          openOtherRef.current(file);
        } else notify(t("fm.chooseProject"), "error");
        return;
      }
      if (format === "lbrn2") {
        // LightBurn files need a field size and a replace/merge choice.
        setModal({ kind: "import", files: [file], mode: lbrn2Mode ?? defaultImportMode(live.current.doc) });
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        notify(t("fm.tooLarge", { name: file.name, size: formatBytes(MAX_UPLOAD_BYTES) }), "error");
        return;
      }
      guard(t("fm.opening", { name: file.name }), t("fm.discardOpen"), () => {
        setModal(null);
        void task(async () => {
          const result = commandResult(await api.upload(file, "openetch", "replace", undefined, { name: file.name }).catch((e: unknown) => Promise.reject(fileError(e, file.name))));
          detach(suggestProjectName(fileStem(file.name)));
          setLoadedUnsaved(true);
          setImportedFieldMm(null);
          setOpenedName(fileStem(file.name));
          notify(t("fm.openedFile", { name: file.name }));
          showWarnings(t("fm.opening", { name: file.name }), warningList(result));
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, detach, notify, showWarnings, t],
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
              if (file.size > MAX_UPLOAD_BYTES) throw new Error(t("fm.tooLarge", { name: file.name, size: formatBytes(MAX_UPLOAD_BYTES) }));
              // No size: the engine picks it (the file's LightBurn device first) and reports it with `field_source`.
              const result = commandResult(await api.upload(file, "lbrn2", mode, fieldMm, { name: fileStem(file.name) }).catch((e: unknown) => Promise.reject(fileError(e, file.name))));
              const outcome = importOutcome(result);
              last = outcome;
              objects += outcome.objects;
              created.push(...createdIds(result));
              warnings.push(...outcome.warnings.map((w) => (files.length > 1 ? `${file.name}: ${w}` : w)));
            } catch (e) {
              if (files.length === 1) throw e;
              failed.push(`${file.name}: ${errorText(e)}`);
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
              t(objects === 1 ? "fm.merged1" : "fm.mergedN", { done: files.length - failed.length, total: files.length, objects }),
              failed.length ? "error" : "info",
            );
          for (const f of failed) notify(f, "error");
          showWarnings(files.length > 1 ? t("fm.importOfN", { n: files.length }) : t("fm.importOf", { name: files[0].name }), warnings);
        });
      };
      if (mode === "replace") guard(t("fm.importing", { name: files[0].name }), t("fm.discardImport"), go, { kind: "import", files, mode });
      else go();
    },
    [guard, task, engine, detach, notify, showWarnings, t],
  );

  const baseName = () => exportBaseName(live.current.projectName, live.current.openedName, live.current.doc?.name);

  const exportLbrn2 = useCallback((fieldMm?: number) => {
    setModal(null);
    void task(async () => {
      // Always the size the Export dialog showed, so the file is laid out exactly as the user saw it; no size for a
      // gantry device, whose workspace and origin the engine applies.
      const res = await execute(C.exportLbrn2(fieldMm));
      const r = res.result as { content?: unknown } | null;
      if (!r || typeof r.content !== "string") throw new Error(t("fm.noLbContent"));
      const name = downloadName(baseName(), "lbrn2");
      saveBlob(new Blob([r.content], { type: "application/xml" }), name);
      const w = warningList(r);
      const dev = live.current.active;
      const what =
        fieldMm !== undefined ? t("fm.aField", { mm: fmt(fieldMm, 3) }) : dev ? `${dev.display_name} (${workspaceText(dev.workspace_mm)})` : t("fm.theActiveDevice");
      const warn = w.length ? ` ${t(w.length === 1 ? "fm.warn1" : "fm.warnN", { n: w.length })}` : "";
      notify(t("fm.exported", { name, what, warn }));
      showWarnings(t("fm.exportOf", { name }), w);
    });
  }, [task, notify, showWarnings, t]);

  const downloadOpenetch = useCallback(() => {
    void task(async () => {
      const { blob, filename } = await api.download("openetch").catch((e: unknown) => Promise.reject(fileError(e, t("fm.download"))));
      // The engine names the file after its project, which this UI may have replaced (see `currentProjectName`).
      const name = (live.current.projectName !== null ? filename : null) ?? downloadName(baseName(), "openetch");
      saveBlob(blob, name);
      notify(t("fm.downloaded", { name }));
    });
  }, [task, notify, t]);

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
  return formatChord("Ctrl+S");
}

const noop = () => {};

/** The text of a menu row element (its first <span>, else all its text). */
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) {
    const first = node.find((n) => isValidElement(n) && (n as ReactElement).type === "span");
    return first ? textOf(first) : node.map(textOf).join("");
  }
  if (isValidElement(node)) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

/**
 * The File menu entries the other layers contribute (App passes them as render functions): FileMenuPart data, the
 * workspace layer's Preferences / Edit hotkeys, and plain menu rows (New tab), filed by section.
 */
export function collectFileParts(node: ReactNode, t: T, out: Partial<Record<FileSection, MenuItem[]>> = {}): Partial<Record<FileSection, MenuItem[]>> {
  const add = (s: FileSection, items: MenuItem[]) => {
    out[s] = [...(out[s] ?? []), ...items];
  };
  const walk = (n: ReactNode) => {
    if (Array.isArray(n)) return n.forEach(walk);
    if (!isValidElement(n)) return;
    const el = n as ReactElement<Record<string, unknown>>;
    if (el.type === Fragment) return walk(el.props.children as ReactNode);
    if (el.type === FileMenuPart) {
      const pp = el.props as unknown as { section: FileSection; items: MenuItem[] };
      return add(pp.section, pp.items);
    }
    if (el.type === WorkspaceFileItems) {
      const pp = el.props as unknown as { open: (d: WsDialog) => void };
      return add("settings", workspaceFileMenuItems(pp.open, t));
    }
    if (el.props.role === "menuitem") {
      const onClick = el.props.onClick as ((e: unknown) => void) | undefined;
      const label = textOf(el.props.children as ReactNode);
      // data-action-id: the hotkeys.ts action the row runs, so command search lists it once.
      const actionId = typeof el.props["data-action-id"] === "string" ? (el.props["data-action-id"] as string) : undefined;
      return add("top", [{ label, actionId, disabled: !!el.props.disabled, action: onClick ? () => onClick({}) : undefined }]);
    }
    // Anything else is shown as it is.
    add("top", [{ label: "", content: () => el }]);
  };
  walk(node);
  return out;
}

/**
 * The File button and its menu (Menu primitive; structure in fileMenuModel.ts): New, Open, Open Recent ▸, Save, Save
 * As, Import, Export ▸, Project ▸, LightBurn ▸, Laser ▸ (Devices, Material Library, Find My Laser, Batch run, Job
 * log), New tab, Settings. `onImport` opens the Import picker (also Ctrl+I or a drop); `onDevices` opens the Devices
 * window (absent on engines without the device registry).
 */
export function FileMenu({
  label,
  file,
  onImport,
  onDevices,
  recent,
  extra,
  files,
  onSaveGcode,
  onSaveRd,
}: {
  /** The button's text (default "File"; the phone's More > File section names it "All file commands"). */
  label?: string;
  file: FileActions;
  onImport: () => void;
  onDevices?: () => void;
  /** Save G-code (LightBurn's Save GCode): given only while the active device is a GRBL gantry. */
  onSaveGcode?: () => void;
  /** Save RD file (LightBurn's Save RD file): given only while the active device is a Ruida gantry. */
  onSaveRd?: () => void;
  /** Recent projects (workspace/RecentProjects.tsx), given a function that closes the menu. */
  recent?: (close: () => void) => ReactNode;
  /** More entries (Preferences, Edit hotkeys, New tab, the Laser entries), given a function that closes the menu. */
  extra?: (close: () => void) => ReactNode;
  /** Export, Print, templates, notes, backups... (src/files/useFilesUi.tsx). */
  files?: (close: () => void) => ReactNode;
}) {
  const t = useT();
  const parts = collectFileParts(files?.(noop), t, collectFileParts(extra?.(noop), t));
  const items = buildFileMenu({
    busy: file.busy,
    newDocument: file.newDocument,
    open: () => file.setModal({ kind: "open" }),
    recent: recent ? { label: t("rb.openRecent"), content: (close) => recent(close) } : undefined,
    save: file.save,
    saveAs: () => file.setModal({ kind: "saveAs" }),
    importFiles: onImport,
    importLightBurn: file.openLbrn2Dialog,
    exportLightBurn: () => file.setModal({ kind: "export" }),
    downloadOpenetch: file.downloadOpenetch,
    saveGcode: onSaveGcode,
    saveRd: onSaveRd,
    devices: onDevices,
    parts,
    labels: {
      new: t("fm.new"),
      open: t("fm.open"),
      openRecent: t("rb.openRecent"),
      save: t("fm.save"),
      saveAs: t("fm.saveAs"),
      import: t("fm.import"),
      export: t("rb.export"),
      exportLightBurn: t("fm.exportLb"),
      saveGcode: t("fm.saveGcode"),
      saveRd: t("fm.saveRd"),
      downloadOpenetch: t("fm.downloadOpenetch"),
      project: t("rb.project"),
      lightburn: t("rb.lightburn"),
      importLightBurn: t("fm.importLb"),
      laser: t("rb.laserMenu"),
      devices: t("fm.devices"),
      settings: t("rb.settings"),
    },
  });
  usePublish("menu.file", { label: t("fm.file"), items });
  return (
    <div className="file-menu">
      <MenuButton className="file-btn" label={label ?? t("fm.file")} items={items}>
        {label ?? t("fm.file")}
        <Icon name="chevronDown" size={16} />
      </MenuButton>
    </div>
  );
}

/**
 * Project name and unsaved-changes dot for the top bar. A long name is cut with "…", so the name is a button: hover
 * shows the whole name, and a click or tap opens it in full with Save as… (to give it a new name).
 */
export function ProjectIndicator({ name, docName, dirty, onSaveAs }: { name: string | null; docName: string | null; dirty: boolean | null; onSaveAs?: () => void }) {
  const t = useT();
  const label = name ?? docName;
  const lines = [name ? t("fm.projectNamed", { name }) : t("fm.notProject"), dirty ? t("fm.unsavedTitle") : dirty === false ? t("fm.allSaved") : ""].filter(Boolean);
  if (!label && !dirty) return null;
  const items: MenuItem[] = [
    {
      label: label ?? t("fm.notProject"),
      content: () => (
        <div className="project-pop">
          {label && <div className="project-pop-name">{label}</div>}
          {lines.map((l) => (
            <div key={l} className="project-pop-state">
              {l}
            </div>
          ))}
        </div>
      ),
    },
  ];
  if (onSaveAs) items.push({ sep: true }, { label: t("fm.saveAs"), icon: "save", action: onSaveAs });
  return (
    <span className="project">
      <MenuButton className="project-btn" label={label ?? t("fm.notProject")} title={lines.join("\n")} ariaLabel={lines.join(". ")} items={items}>
        {label && <span className={`doc-name${name ? "" : " unsaved"}`}>{label}</span>}
        {dirty && <span className="dirty-dot" role="img" aria-label={t("fm.unsavedTitle")} />}
      </MenuButton>
    </span>
  );
}

/** Every File dialog plus the warnings panel, rendered into <body> so they do not inherit the top bar's styles. */
export function FileDialogs({ file, docName }: { file: FileActions; docName: string | null }) {
  const t = useT();
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
                {t("common.cancel")}
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
  const t = useT();
  const rows = groupWarnings(p.items);
  // Placed by the canvas HUD after its controls (bottom left), so it never covers one.
  const ref = useHudFixed<HTMLDivElement>({ id: "import-warnings", slot: "bl", rank: 95, minW: 240, late: true });
  return (
    <div ref={ref} className="warn-panel" role="status" aria-live="polite">
      <div className="warn-panel-head">
        <span>
          {p.title}: {t(p.items.length === 1 ? "fm.warning1" : "fm.warningN", { n: p.items.length })}
        </span>
        <button className="icon-btn small" onClick={p.onClose} aria-label={t("fm.dismissWarnings")}>
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
        setError(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, []);
  return { list, error };
}

function OpenDialog({ file, onClose }: { file: FileActions; onClose: () => void }) {
  const t = useT();
  const { list, error } = useProjectList();
  const tree = useProjectFolders();
  const [folder, setFolder] = useState(() => folderOf(file.projectName ?? ""));
  const names = useMemo(() => (list ?? []).map((p) => p.name), [list]);
  // Engines without folders (or with none yet) show the flat list as before.
  const useTree = tree.folders !== null && (tree.folders.length > 0 || tree.locations.length > 0 || names.some((n) => n.includes("/")));
  const shown = list === null ? null : useTree ? projectsIn(list, folder) : list;
  const input = useRef<HTMLInputElement>(null);
  const projects = (
    <div className="folder-projects">
      {shown === null && <p className="muted">{t("common.loading")}</p>}
      {error && <p className="form-error">{error}</p>}
      {shown !== null && !error && shown.length === 0 && <p className="muted">{useTree && list?.length ? t("fold.emptyFolder") : t("fm.noProjects")}</p>}
      {shown !== null && shown.length > 0 && (
        <div className="project-list">
          {shown.map((pr) => (
            <button
              key={pr.name}
              className={`project-row with-thumb${pr.name === file.projectName ? " current" : ""}`}
              disabled={file.busy}
              title={pr.name}
              onClick={() => file.openProject(pr.name)}
            >
              <ProjectThumb name={pr.name} modified={pr.modified_unix_s} />
              <span className="project-row-text">
                <span className="project-row-name">{useTree ? baseNameOf(pr.name) : pr.name}</span>
                <span className="project-row-meta">
                  {[formatModified(pr.modified_unix_s), formatBytes(pr.size)].filter(Boolean).join(" · ")}
                </span>
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
  return (
    <Modal
      title={t("fm.openTitle")}
      wide={useTree}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={() => input.current?.click()} disabled={file.busy}>
            <Icon name="upload" /> {t("fm.upload")}
          </button>
          <button className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
        </>
      }
    >
      <input
        ref={input}
        type="file"
        accept={pickerAccept(`.openetch,.lbrn2,${ENGINE_ACCEPT}`)}
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          // The user chose Open: a LightBurn file replaces the design.
          if (f) file.uploadFile(f, "replace");
        }}
      />
      <div className="section-label">{t("fm.projectsOnEngine")}</div>
      {useTree && tree.folders ? (
        <div className="folder-browser">
          <FolderTree
            folders={tree.folders}
            locations={tree.locations}
            projectNames={names}
            selected={folder}
            disabled={file.busy}
            onSelect={setFolder}
            onCreated={(f) => (tree.reload(), setFolder(f))}
          />
          {projects}
        </div>
      ) : (
        projects
      )}
      <p className="hint">{t("fm.uploadHint")}</p>
    </Modal>
  );
}

function SaveAsDialog({ file, docName, onClose }: { file: FileActions; docName: string | null; onClose: () => void }) {
  const t = useT();
  const initial = file.suggestedName || file.projectName || (docName && docName !== "Untitled" ? suggestProjectName(docName) : "");
  // The folder is picked in the tree; a name typed with "/" still works (it adds folders under the picked one).
  const [folder, setFolder] = useState(() => folderOf(initial));
  const [name, setName] = useState(() => baseNameOf(initial));
  const tree = useProjectFolders();
  const [confirmReplace, setConfirmReplace] = useState(false);
  // The engine refused the save because a project of that name exists that the list did not show (case-only clash,
  // or saved meanwhile from elsewhere).
  const [refused, setRefused] = useState<string | null>(null);
  const { list } = useProjectList();
  const names = useMemo(() => (list ?? []).map((p) => p.name), [list]);
  const trimmed = name.trim() ? joinProject(folder, name.trim()) : "";
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
      title={t("fm.saveAsTitle")}
      wide={tree.folders !== null}
      onClose={onClose}
      footer={
        <>
          {problem && <span className="form-error">{problem}</span>}
          <button className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className={`btn ${confirmReplace ? "btn-warn" : "btn-primary"}`} disabled={!trimmed || !!problem || file.busy} onClick={() => void submit()}>
            {confirmReplace ? t("fm.replace") : t("fm.save")}
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">{t("fm.projectName")}</span>
        <span className="field-input">
          <input
            type="text"
            autoFocus
            value={name}
            maxLength={200}
            placeholder={t("fm.namePlaceholder")}
            onChange={(e) => {
              setName(e.target.value);
              setConfirmReplace(false);
            }}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
        </span>
      </label>
      {tree.folders !== null && (
        <>
          <div className="section-label">{t("fold.saveIn", { folder: folder || t("fold.root") })}</div>
          <FolderTree
            folders={tree.folders}
            locations={tree.locations}
            projectNames={names}
            selected={folder}
            disabled={file.busy}
            onSelect={(f) => (setFolder(f), setConfirmReplace(false))}
            onCreated={(f) => (tree.reload(), setFolder(f), setConfirmReplace(false))}
          />
        </>
      )}
      {existing !== null && <p className="hint warn-text">{t("fm.exists", { name: existing })}</p>}
      <p className="hint">{t("fm.savedAs", { file: trimmed && !problem ? `"${trimmed}.openetch"` : t("fm.nameFile") })}</p>
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
  const t = useT();
  const n = parseField(p.value);
  return (
    <>
      <label className="field">
        <span className="field-label">{t("fm.fieldSize")}</span>
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
  const t = useT();
  const rounded = p.machineMm != null && !p.workspaceMm && sameField(p.mm, p.machineMm) && Math.abs(p.mm - p.machineMm) >= 1e-3;
  return (
    <p className="field-summary">
      <span>
        {p.workspaceMm ? (
          <>
            {t("fm.workspace")} <strong>{workspaceText(p.workspaceMm)}</strong>
          </>
        ) : (
          <>
            {t("fm.fieldSize")} <strong>{fmt(p.mm, 3)} mm</strong>
          </>
        )}{" "}
        — {p.source}
        {rounded && <span className="muted"> {t("fm.rounding", { mm: fmt(p.machineMm as number, 3) })}</span>}
      </span>
      {p.onEdit && !p.workspaceMm && (
        <button className="link-btn" onClick={p.onEdit}>
          {t("fm.edit")}
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
function fieldChoiceList(t: T, file: FileActions, first: { mm: number | null; label: string | null }[] = []): FieldChoice[] {
  return labelledFieldChoices([
    ...first,
    // Galvo fields only: a gantry workspace is not a field size.
    ...file.lightburnDevices.filter((d) => d.kind === undefined || isGalvo(d.kind)).map((d) => ({ mm: d.field_mm, label: d.name })),
    { mm: file.docFieldMm, label: t("fm.currentDesign") },
    { mm: file.machineFieldMm, label: t("fm.thisLaser") },
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
  const t = useT();
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
  const error = field !== null && typed === null ? t("fm.fieldError") : null;
  const ready = picked.length > 0 && !reading && error === null && !file.busy;
  const submit = () => {
    if (ready) file.importLbrn2(picked, effective, typed ?? undefined);
  };
  const edit = () => setField(fmt((shared ?? defaults[0]).fieldMm, 3));
  return (
    <Modal
      title={t("fm.lbTitle")}
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" disabled={!ready} onClick={submit}>
            {effective === "replace" ? t("fm.openBtn") : t("fm.importBtn")}
          </button>
        </>
      }
    >
      <input
        ref={input}
        type="file"
        accept={pickerAccept(".lbrn2")}
        multiple
        hidden
        onChange={(e) => {
          // iOS offers every file (no accept there, touch/filePick.ts): only the LightBurn files are kept.
          const f = splitPicked(Array.from(e.target.files ?? []), ".lbrn2").ok;
          e.target.value = "";
          if (f.length) setPicked(f);
        }}
      />
      <div className="field">
        <span className="field-label">{several ? t("fm.files") : t("fm.fileLabel")}</span>
        <button className="btn file-pick" onClick={() => input.current?.click()}>
          <Icon name="upload" />
          <span className="file-pick-name">
            {picked.length === 0
              ? t("fm.chooseLb")
              : several
                ? t("fm.nFiles", { n: picked.length, size: formatBytes(picked.reduce((a, f) => a + f.size, 0)) })
                : `${picked[0].name} (${formatBytes(picked[0].size)})`}
          </span>
        </button>
      </div>
      {several && <p className="hint">{picked.map((f) => f.name).join(", ")}</p>}
      <div className="segmented" role="radiogroup" aria-label={t("fm.openOrImport")}>
        <button
          role="radio"
          aria-checked={effective === "replace"}
          className={effective === "replace" ? "active" : ""}
          disabled={several}
          title={several ? t("fm.severalTitle") : t("fm.replaceTitle")}
          onClick={() => setMode("replace")}
        >
          {t("fm.openReplace")}
        </button>
        <button
          role="radio"
          aria-checked={effective === "merge"}
          className={effective === "merge" ? "active" : ""}
          title={t("fm.mergeTitle")}
          onClick={() => setMode("merge")}
        >
          {t("fm.importMerge")}
        </button>
      </div>
      <p className="hint">
        {t("fm.openImportHint")}
        {several ? ` ${t("fm.severalHint")}` : ""}
      </p>
      {field === null ? (
        reading ? (
          <p className="field-summary muted">{t("fm.reading")}</p>
        ) : shared ? (
          <FieldSummary mm={shared.fieldMm} source={fieldSourceText(shared)} onEdit={edit} workspaceMm={shared.workspaceMm} machineMm={p.file.machineFieldMm} />
        ) : (
          <>
            <p className="field-summary">
              <span>{t("fm.eachOwn")}</span>
              <button className="link-btn" onClick={edit}>
                {t("fm.edit")}
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
            choices={fieldChoiceList(t, file, defaults.map((d) => ({ mm: d.fieldMm, label: d.device })))}
          />
          <p className="hint">
            {t("fm.noFieldInFile")}
          </p>
          <button className="link-btn" onClick={() => setField(null)}>
            {t("fm.autoSize")}
          </button>
        </>
      )}
      {field === null && !reading && unknown.length > 0 && (
        <p className="hint">
          {unknown.length === 1 ? t("fm.madeFor1", { name: unknown[0] }) : t("fm.madeForN", { names: unknown.map((u) => `"${u}"`).join(", ") })}
          {file.lightburnDeviceKind === "active-device" ? t("fm.notYourDevice") : t("fm.engineUnknown")}
        </p>
      )}
    </Modal>
  );
}

/** Export to LightBurn: shows the field size the project will be laid out for, and lets it be changed. */
function ExportDialog({ file, onClose }: { file: FileActions; onClose: () => void }) {
  const t = useT();
  const [field, setField] = useState<string | null>(null);
  const typed = field === null ? null : parseField(field);
  const error = field !== null && typed === null ? t("fm.fieldError") : null;
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
      title={t("fm.exportTitle")}
      onClose={onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn btn-primary" autoFocus={field === null} disabled={(!gantry && fieldMm === null) || file.busy} onClick={submit}>
            {t("fm.export")}
          </button>
        </>
      }
    >
      {gantry && active ? (
        <>
          <FieldSummary mm={active.workspace_mm[0]} source={t("fm.fromActive", { name: active.display_name })} workspaceMm={active.workspace_mm} />
          <p className="hint">
            {t("fm.laidOut", { display: active.display_name, name: active.name, origin: originLabel(active.origin).toLowerCase() })}
          </p>
        </>
      ) : field === null ? (
        <>
          <FieldSummary mm={file.exportFieldMm} source={exportFieldOriginText(origin)} onEdit={() => setField(fmt(file.exportFieldMm, 3))} machineMm={file.machineFieldMm} />
          {active && !sameField(active.workspace_mm[0], file.exportFieldMm) && (
            <p className="hint">{t("fm.activeDevice", { name: active.display_name, size: workspaceText([active.workspace_mm[0], active.workspace_mm[0]]) })}</p>
          )}
          {deviceDiffers && device && (
            <p className="hint">
              {t(active ? "fm.activeUses" : "fm.lbUses", { name: device.name, mm: fmt(device.field_mm, 3) })}
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
            choices={fieldChoiceList(t, file, [{ mm: file.importedFieldMm, label: t("fm.openedWith") }])}
          />
          <p className="hint">
            {t("fm.noFieldExport")}
          </p>
          <button className="link-btn" onClick={() => setField(null)}>
            {t("fm.autoSize")}
          </button>
        </>
      )}
    </Modal>
  );
}
