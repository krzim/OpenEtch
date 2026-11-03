// The files and image-tools layer of the UI, as one hook for App.tsx (LightBurn's File menu, Image Options and
// Tools > Trace Image / Convert to Bitmap; routes in crates/engine/src/userfiles.rs, commands in crates/commands/src/
// builtin/{files,image_tools,pages}.rs):
// - File menu entries: Export (Alt+X), Print, Export image / Save processed bitmap, templates (open, recent, Save As
//   Template), Notes (Ctrl+Alt+N), Restore from autosave / backups with the autosave status, Import options,
//   Preferences and bundles.
// - Imports of every further format (PDF, AI, EPS, HPGL/PLT, Gerber/Excellon, 3D models, TIFF/TGA, EzCad, legacy
//   .lbrn, .lbt), picked or dropped: `intercept` takes them (and SVG/DXF/bitmaps, so the import options apply to all)
//   before App's router; 3D models and Gerber files open the options dialog; File > Open opens them as a new project.
// - Image tools on the selection (context menu and hotkeys): Trace (Alt+T), Adjust image (Alt+I), Convert to bitmap,
//   Apply / Remove / Flatten mask, Crop to mask, Replace / Replace to fit / Refresh image, Export image, Save processed
//   bitmap; and "Move selection to page".
// - The Pages dock, the Notes window (shown on open when the project asks for it).
// Engines without these commands get none of it (each piece checks `has`).

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { DESIGN_ONLY } from "../mode.ts";
import { bytesToBase64, createdIds } from "../api/drop.ts";
import { IMAGE_ADJUST_CMD, imageAdjust, imageUpdate } from "../api/image.ts";
import { commandResult, formatBytes, needsConfirm, warningList } from "../api/project.ts";
import type { Engine } from "../api/useEngine.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import { FileMenuPart } from "../components/FileMenuPart.tsx";
import type { FileActions } from "../components/FileMenu.tsx";
import { AdjustImageDialog } from "../components/ImageSettings.tsx";
import { Modal } from "../components/ui.tsx";
import { unitsOf } from "../selection.ts";
import { hintFor } from "../workspace/hotkeys.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import type { KeyHandler } from "../workspace/useKeymap.ts";
import { ExportDialog, PrintDialog } from "./ExportDialogs.tsx";
import {
  MAX_IMPORT_BYTES,
  autosaveStatus,
  backupLists,
  engineFormatOf,
  extOf,
  fileSettingsOf,
  imageFileName,
  importQuery,
  importTargetLayer,
  liveRecent,
  maskOf,
  maskPair,
  needsOptions,
  notesOf,
  notesWait,
  openedRevision,
  resultGroup,
  editModelInvocation,
  modelOptionsOf,
  selectedImages,
  selectedVectors,
  showNotesOnLoad,
  templateList,
  ungroupAfter,
  type EngineFormat,
  type ModelOptions,
  type Notes,
  type TemplateEntry,
} from "./fileFormats.ts";
import { appModeDialogOpen } from "../app/AppModeDialog.tsx";
import { GUIDE_EVENT } from "../guide/route.ts";
import { filesApi, saveBlob } from "./filesApi.ts";
import { filesState, rememberTemplate, useFilesStore } from "./filesStore.ts";
import { FilePick } from "./FilesUiParts.tsx";
import { ConvertBitmapDialog, IMAGE_CMD, TraceDialog } from "./ImageTools.tsx";
import { EditModelDialog, ImportFilesDialog, ImportOptionsDialog, type ImportChoice } from "./ImportDialogs.tsx";
import { PagesDock, sendToPageItems } from "./PagesDock.tsx";
import { BackupsDialog, NotesWindow, PrefsBundlesDialog, SaveTemplateDialog, TemplatesDialog } from "./ProjectDialogs.tsx";
import "./files.css";

type Notify = (msg: string, kind?: "info" | "error") => void;

type FilesModal =
  | { kind: "export" }
  | { kind: "print"; colors?: "layers" | "black" }
  | { kind: "import"; files: File[]; mode: "merge" | "replace" }
  | { kind: "importOptions" }
  | { kind: "backups" }
  | { kind: "templates" }
  | { kind: "saveTemplate" }
  | { kind: "prefs" }
  | { kind: "notes" }
  | { kind: "trace"; id: string }
  | { kind: "toBitmap"; ids: string[] }
  | { kind: "adjust"; layer: string; ids: string[] }
  | { kind: "editModel"; id: string; name: string; options: ModelOptions }
  | { kind: "confirm"; message: string; label: string; action: () => void };

export const NOTES_CMD = "doc.set_notes";

export interface FilesUi {
  /** File menu entries, given a function that closes the menu. */
  menu: (close: () => void) => ReactNode;
  /** Canvas context menu entries for the selection. */
  contextItems: MenuItem[];
  /** Workspace hotkey handlers (workspace/hotkeys.ts ids). */
  handlers: Record<string, KeyHandler>;
  dialogs: ReactNode;
  /** The Pages dock for the right panel (null on engines without pages). */
  pagesDock: ReactNode;
  /** Takes the files it imports itself and returns the rest for App's router. */
  intercept: (files: File[]) => File[];
  /** File > Open of a file that is not a project: open it as a new project. */
  openOther: (file: File) => void;
  /** One of its dialogs is open (keys and drops wait). */
  modalOpen: boolean;
  /** Open the Templates dialog (File > Project > Templates; the empty-canvas card's Templates button). */
  openTemplates: () => void;
}

export function useFilesUi(p: {
  engine: Engine;
  file: FileActions;
  selection: string[];
  selectIds: (ids: string[]) => void;
  activeLayer: string | null;
  design: boolean;
  /** The layout mode (workspace/layoutMode.ts): whether the guide is a full sheet, which notes wait for. */
  layout: string;
  notify: Notify;
  report: (e: unknown) => void;
}): FilesUi {
  const t = useT();
  const hotkeys = usePrefs().hotkeys;
  const { engine, notify, report } = p;
  const { doc, has, run } = engine;
  const [modal, setModal] = useState<FilesModal | null>(null);
  const [busy, setBusy] = useState(false);
  const close = useCallback(() => setModal(null), []);
  const live = useRef({ doc, dirty: p.file.dirty, selection: p.selection, activeLayer: p.activeLayer });
  live.current = { doc, dirty: p.file.dirty, selection: p.selection, activeLayer: p.activeLayer };
  const can = {
    imports: C.hasFileImports(has, DESIGN_ONLY),
    exports: has("doc.export"),
    print: has("doc.print"),
    trace: has(IMAGE_CMD.trace),
    toBitmap: has(IMAGE_CMD.toBitmap),
    mask: has(IMAGE_CMD.applyMask),
    replace: has(IMAGE_CMD.replace),
    imageExport: has(IMAGE_CMD.exportImage),
    notes: has(NOTES_CMD),
    adjust: has(IMAGE_ADJUST_CMD),
    editModel: has("image.edit_model"),
  };

  // ---- notes shown on open (LightBurn "Show notes when the file opens") ----------------------------------------
  const notes: Notes = notesOf(doc);
  const notesRef = useRef(notes);
  notesRef.current = notes;
  // Opening anything (here, File > Open, a LightBurn file, a template, a backup, an agent's doc.load) starts a fresh
  // timeline whose one step loads it; the notes show once per opened revision, and once for what this page finds.
  const opened = openedRevision(engine.timeline);
  const shownFor = useRef<string | null>(null);
  const layoutRef = useRef(p.layout);
  layoutRef.current = p.layout;
  const [notesWaiting, setNotesWaiting] = useState(false);
  useEffect(() => {
    if (!doc) return;
    // Keyed by revision, so the page's first document and the open that produced it count once.
    const cur = opened !== null && opened === doc.revision ? String(opened) : null;
    const key = shownFor.current === null ? (cur ?? String(doc.revision)) : (cur ?? shownFor.current);
    const prev = shownFor.current;
    shownFor.current = key;
    if (!showNotesOnLoad(prev, key, notesRef.current)) return;
    if (notesWait(location.hash, layoutRef.current, window.innerWidth)) setNotesWaiting(true);
    else setModal((m) => m ?? { kind: "notes" });
  }, [doc, opened]);
  // Not over the production cockpit or the guide as a full sheet (fileFormats.ts notesWait): the notes wait until
  // the workspace is back, then show if the document still has them. Opening and closing those routes does not
  // always fire an event (pushState / replaceState), so a slow poll backs the events up.
  useEffect(() => {
    if (!notesWaiting) return;
    const check = () => {
      // Nor on top of the "Use as an app" offer, which may come up as the cockpit closes (it waits for the notes too).
      if (notesWait(location.hash, layoutRef.current, window.innerWidth) || appModeDialogOpen()) return;
      setNotesWaiting(false);
      const n = notesRef.current;
      if (n.show_on_load && n.text.trim()) setModal((m) => m ?? { kind: "notes" });
    };
    const events = ["hashchange", "popstate", "resize", GUIDE_EVENT];
    for (const e of events) window.addEventListener(e, check);
    const poll = window.setInterval(check, 1000);
    return () => {
      for (const e of events) window.removeEventListener(e, check);
      window.clearInterval(poll);
    };
  }, [notesWaiting]);

  /** Run `action` now, or after confirming that unsaved changes are replaced. */
  const guard = useCallback(
    (what: string, label: string, action: () => void) => {
      if (!needsConfirm(live.current.dirty, live.current.doc)) return action();
      setModal({ kind: "confirm", message: t("files.unsavedReplace", { what }), label, action });
    },
    [t],
  );

  const afterOpen = useCallback(async () => {
    await engine.refreshDoc();
    await engine.refreshProject();
  }, [engine]);

  // ---- imports -------------------------------------------------------------------------------------------------

  const importFiles = useCallback(
    (files: File[], choice: Partial<ImportChoice> & { mode: "merge" | "replace" }) => {
      const run1 = async () => {
        setModal(null);
        setBusy(true);
        p.file.setBusy(true);
        const prefs = filesState().importPrefs;
        const created: string[] = [];
        const warnings: string[] = [];
        const done: string[] = [];
        try {
          for (const f of files) {
            const format = engineFormatOf(f.name);
            try {
              if (!format) throw new Error(t("files.cannotImport", { ext: extOf(f.name) || "?" }));
              if (C.unavailableImports(has, DESIGN_ONLY).includes(format)) throw new Error(t("dm.importUnavailable", { ext: format.toUpperCase() }));
              if (f.size > MAX_IMPORT_BYTES) throw new Error(t("files.tooLarge", { size: formatBytes(MAX_IMPORT_BYTES) }));
              if (f.size === 0) throw new Error(t("files.empty"));
              const q = importQuery({
                format,
                fileName: f.name,
                mode: files.length > 1 ? "merge" : choice.mode,
                layer: importTargetLayer(live.current.doc?.layers, live.current.activeLayer, prefs.toolLayers),
                prefs,
                widthMm: choice.widthMm ?? null,
                page: choice.page ?? null,
                unitsPerMm: choice.unitsPerMm ?? null,
                gerber: choice.gerber,
                model: choice.model,
              });
              const result = commandResult(await filesApi.importFile(q, f));
              const ids = createdIds(result);
              const g = resultGroup(result);
              if (g && ungroupAfter(format, prefs) && has(C.OPT.groupUngroup)) await run(C.groupUngroup(g)).catch(() => undefined);
              created.push(...ids);
              warnings.push(...warningList(result).map((w) => (files.length > 1 ? `${f.name}: ${w}` : w)));
              done.push(f.name);
            } catch (e) {
              report(new Error(`${f.name}: ${e instanceof Error ? e.message : String(e)}`));
            }
          }
          if (choice.mode === "replace" && done.length) await afterOpen();
          else await engine.refreshDoc();
          if (created.length && prefs.select) p.selectIds(created);
          if (done.length === 1) notify(choice.mode === "replace" ? t("files.openedAsNew", { name: done[0] }) : t("files.imported1", { name: done[0], n: created.length }));
          else if (done.length > 1) notify(t("files.importedN", { n: done.length, objects: created.length }));
          if (warnings.length) notify(warnings.slice(0, 3).join(" · ") + (warnings.length > 3 ? ` (+${warnings.length - 3})` : ""));
        } finally {
          setBusy(false);
          p.file.setBusy(false);
        }
      };
      if (choice.mode === "replace") guard(files[0]?.name ?? "", t("files.discardOpen"), () => void run1());
      else void run1();
    },
    [p.file, p.selectIds, t, has, run, report, engine, notify, guard, afterOpen],
  );

  /** Formats handled here (all the engine imports, so the import options apply to every one). */
  const mine = useCallback((f: File): EngineFormat | null => (can.imports ? engineFormatOf(f.name) : null), [can.imports]);

  const intercept = useCallback(
    (files: File[]): File[] => {
      const take = files.filter((f) => mine(f) !== null);
      if (take.length === 0) return files;
      const rest = files.filter((f) => mine(f) === null);
      // A LightBurn template opens as a new untitled copy (LightBurn's Open of an .lbt).
      const templates = take.filter((f) => mine(f) === "lbt");
      const art = take.filter((f) => mine(f) !== "lbt");
      if (templates.length) importFiles(templates.slice(0, 1), { mode: "replace" });
      if (art.length) {
        if (art.some((f) => needsOptions(mine(f) as EngineFormat))) setModal({ kind: "import", files: art, mode: "merge" });
        else importFiles(art, { mode: "merge" });
      }
      return rest;
    },
    [mine, importFiles],
  );

  const openOther = useCallback(
    (f: File) => {
      const format = engineFormatOf(f.name);
      if (!format || !can.imports) {
        notify(t("files.cannotOpen", { name: f.name }), "error");
        return;
      }
      if (needsOptions(format)) setModal({ kind: "import", files: [f], mode: "replace" });
      else importFiles([f], { mode: "replace" });
    },
    [can.imports, notify, t, importFiles],
  );

  // ---- templates -----------------------------------------------------------------------------------------------

  const openTemplate = useCallback(
    (name: string) => {
      guard(name, t("files.discardOpen"), () => {
        setModal(null);
        void filesApi
          .openTemplate(name)
          .then(async () => {
            rememberTemplate(name);
            await afterOpen();
            // LightBurn opens a template as an Untitled document; the engine keeps the template's own document name.
            if (has("doc.rename")) await run({ id: "doc.rename", params: { name: "Untitled" } }).catch(() => undefined);
            notify(t("files.templateOpened", { name }));
          })
          .catch(report);
      });
    },
    [guard, t, afterOpen, notify, report, has, run],
  );

  const restore = useCallback(
    (kind: "autosave" | "backup", fileName: string) => {
      guard(fileName, t("files.discardOpen"), () => {
        setModal(null);
        void filesApi
          .restoreBackup(kind, fileName)
          .then(async () => {
            await afterOpen();
            notify(t("files.restored"));
          })
          .catch(report);
      });
    },
    [guard, t, afterOpen, notify, report],
  );

  // ---- image tools ---------------------------------------------------------------------------------------------

  const images = selectedImages(doc, p.selection);
  const image = images[0] ?? null;
  const vectors = selectedVectors(doc, p.selection);
  const units = doc ? unitsOf(doc, p.selection) : [];
  const pair = doc ? maskPair(units, (id) => doc.objects.find((o) => o.id === id)) : null;
  const masked = images.filter((o) => maskOf(doc, o.id) !== null);

  const replacePick = useRef<HTMLInputElement>(null);
  const replaceMode = useRef<{ id: string; fit: "file_size" | "fit" | "stretch" } | null>(null);
  const replaceImage = (fit: "file_size" | "fit" | "stretch") => {
    if (!image) return;
    replaceMode.current = { id: image.id, fit };
    replacePick.current?.click();
  };
  const onReplaceFile = (files: File[]) => {
    const target = replaceMode.current;
    const f = files[0];
    if (!target || !f) return;
    void (async () => {
      const bytes = new Uint8Array(await f.arrayBuffer());
      if (bytes.length > 20 * 1024 * 1024) throw new Error(t("files.tooLarge", { size: "20 MB" }));
      await run({ id: IMAGE_CMD.replace, params: { id: target.id, data_base64: bytesToBase64(bytes), fit: target.fit } });
      notify(t("files.imageReplaced", { name: f.name }));
    })().catch(report);
  };

  const downloadImage = useCallback(
    (processed: boolean) => {
      const o = image;
      if (!o) return notify(t("files.selectImage"), "error");
      void (processed ? filesApi.imageProcessed(o.id) : filesApi.imageFile(o.id))
        .then((d) => {
          const name = d.filename ?? imageFileName(o.name, processed ? "png" : "png", processed ? " processed" : "");
          saveBlob(d.blob, name);
          notify(t("files.downloaded", { name }));
        })
        .catch(report);
    },
    [image, notify, t, report],
  );

  const exec = (inv: Invocation) => void run(inv).catch(report);

  const trace = () => {
    if (!can.trace) return false;
    if (!image) return void notify(t("files.selectImage"), "error");
    setModal({ kind: "trace", id: image.id });
  };
  const adjust = () => {
    if (!image) return void notify(t("files.selectImage"), "error");
    const same = images.filter((o) => o.layer === image.layer);
    setModal({ kind: "adjust", layer: image.layer, ids: same.map((o) => o.id) });
  };
  const toBitmap = () => {
    if (!can.toBitmap) return false;
    if (vectors.length === 0) return void notify(t("files.selectVectors"), "error");
    setModal({ kind: "toBitmap", ids: vectors.map((o) => o.id) });
  };

  const contextItems: MenuItem[] = [];
  if (p.design && doc) {
    const items: MenuItem[] = [];
    if (image) {
      if (can.trace) items.push({ label: t("files.traceImage"), hint: hintFor("image.trace", hotkeys), action: trace });
      if (can.adjust) items.push({ label: t("files.adjustImage"), hint: hintFor("image.adjust", hotkeys), action: adjust });
      // LightBurn 2.2's Edit 3D Model, for a depth map whose model the engine kept.
      const model = can.editModel && images.length === 1 ? modelOptionsOf(image.extras) : null;
      if (model) items.push({ label: t("files.editModel"), action: () => setModal({ kind: "editModel", id: image.id, name: image.name || image.id, options: model }) });
      if (can.replace) {
        items.push({ label: t("files.replaceImage"), action: () => replaceImage("file_size") });
        items.push({ label: t("files.replaceImageFit"), action: () => replaceImage("fit") });
        items.push({ label: t("files.refreshImage"), action: () => replaceImage("stretch") });
      }
      if (can.imageExport) {
        items.push({ label: t("files.exportImage"), action: () => downloadImage(false) });
        items.push({ label: t("files.saveProcessed"), action: () => downloadImage(true) });
      }
    }
    if (can.mask && pair) items.push({ label: t("files.applyMask"), action: () => exec({ id: IMAGE_CMD.applyMask, params: { ...pair } }) });
    if (can.mask && masked.length) {
      const ids = masked.map((o) => o.id);
      items.push({ label: t("files.removeMask"), action: () => exec({ id: IMAGE_CMD.removeMask, params: { ids } }) });
      items.push({ label: t("files.flattenMask"), action: () => exec({ id: IMAGE_CMD.flattenMask, params: { ids } }) });
      items.push({ label: t("files.cropToMask"), action: () => exec({ id: IMAGE_CMD.flattenMask, params: { ids, crop: true } }) });
    }
    if (can.toBitmap && vectors.length) items.push({ label: t("files.toBitmap"), action: toBitmap });
    const send = sendToPageItems(doc, p.selection, has, run, report, (name) => t("files.sendToPage", { name }));
    // A long canvas menu files these under Modify ▸ (components/contextMenu.ts).
    const tag = (xs: MenuItem[]): MenuItem[] => xs.map((it) => (it.sep ? it : { ...it, section: "modify" }));
    if (items.length) contextItems.push({ sep: true }, ...tag(items));
    if (send.length) contextItems.push({ sep: true }, ...tag(send));
  }

  // ---- hotkeys ---------------------------------------------------------------------------------------------------

  const handlers: Record<string, KeyHandler> = {
    "file.export": () => (can.exports ? setModal({ kind: "export" }) : false),
    "file.print": () => (can.print ? setModal({ kind: "print" }) : false),
    "file.printColors": () => (can.print ? setModal({ kind: "print", colors: "layers" }) : false),
    "window.notes": () => (can.notes ? setModal({ kind: "notes" }) : false),
    "image.trace": () => (p.design ? trace() : false),
    "image.adjust": () => (p.design && can.adjust ? adjust() : false),
    "image.toBitmap": () => (p.design ? toBitmap() : false),
  };

  // ---- menu ------------------------------------------------------------------------------------------------------

  const baseName = p.file.projectName ?? p.file.openedName ?? doc?.name ?? "Untitled";
  // File menu entries as data (components/FileMenuPart.tsx): Export ▸, Project ▸, LightBurn ▸ and Settings.
  const off = p.file.busy || busy;
  const noImage = image ? undefined : t("files.selectImage");
  const menu = (closeMenu: () => void) => (
    <>
      <FileMenuPart
        section="export"
        close={closeMenu}
        items={[
          ...(can.exports ? [{ label: t("files.menuExport"), actionId: "file.export", hint: hintFor("file.export", hotkeys) || "Alt+X", disabled: off, action: () => setModal({ kind: "export" }) }] : []),
          ...(can.print ? [{ label: t("files.menuPrint"), actionId: "file.print", disabled: off, action: () => setModal({ kind: "print" }) }] : []),
          ...(can.imageExport
            ? [
                { label: t("files.exportImage"), disabled: off || !image, title: noImage, action: () => downloadImage(false) },
                { label: t("files.saveProcessed"), disabled: off || !image, title: noImage, action: () => downloadImage(true) },
              ]
            : []),
        ]}
      />
      {can.imports && (
        <>
          <FileMenuPart
            section="project"
            close={closeMenu}
            items={[
              { label: t("files.menuOpenTemplate"), disabled: off, action: () => setModal({ kind: "templates" }) },
              { label: t("files.recentTemplates"), content: (close) => <RecentTemplates disabled={off} onOpen={(n) => (close(), openTemplate(n))} /> },
              { label: t("files.menuSaveTemplate"), disabled: off, action: () => setModal({ kind: "saveTemplate" }) },
              { sep: true },
              ...(can.notes ? [{ label: t("files.menuNotes"), actionId: "window.notes", disabled: off, action: () => setModal({ kind: "notes" }) }] : []),
              { label: t("files.menuBackups"), disabled: off, action: () => setModal({ kind: "backups" }) },
              { label: t("rb.autosave"), content: () => <AutosaveStatus project={p.file.projectName} /> },
            ]}
          />
          <FileMenuPart section="lightburn" close={closeMenu} items={[{ label: t("files.menuPrefs"), disabled: off, action: () => setModal({ kind: "prefs" }) }]} />
          <FileMenuPart section="settings" close={closeMenu} items={[{ label: t("files.menuImportOptions"), disabled: off, action: () => setModal({ kind: "importOptions" }) }]} />
        </>
      )}
    </>
  );

  // ---- dialogs ---------------------------------------------------------------------------------------------------

  const m = modal;
  const traceImage = m?.kind === "trace" ? doc?.objects.find((o) => o.id === m.id) : undefined;
  const adjustLayer = m?.kind === "adjust" ? doc?.layers.find((l) => l.id === m.layer) : undefined;
  const dialogs = (
    <>
      <FilePick ref={replacePick} accept=".png,.jpg,.jpeg,.bmp,.gif,.webp,.tif,.tiff,.tga" onFiles={onReplaceFile} />
      {m?.kind === "export" && <ExportDialog selection={p.selection} baseName={baseName} onClose={close} notify={notify} report={report} />}
      {m?.kind === "print" && <PrintDialog selection={p.selection} baseName={baseName} colors={m.colors} onClose={close} notify={notify} report={report} />}
      {m?.kind === "import" && <ImportFilesDialog files={m.files} initialMode={m.mode} busy={busy} onClose={close} onImport={(files, c) => importFiles(files, c)} />}
      {m?.kind === "importOptions" && <ImportOptionsDialog onClose={close} />}
      {m?.kind === "backups" && <BackupsDialog project={p.file.projectName} onRestore={restore} onClose={close} notify={notify} report={report} />}
      {m?.kind === "templates" && <TemplatesDialog onOpen={openTemplate} onClose={close} notify={notify} report={report} />}
      {m?.kind === "saveTemplate" && (
        <SaveTemplateDialog
          initial={p.file.projectName ?? ""}
          report={report}
          onClose={close}
          onSaved={(name) => {
            rememberTemplate(name);
            notify(t("files.templateSaved", { name }));
            close();
          }}
        />
      )}
      {m?.kind === "prefs" && <PrefsBundlesDialog onClose={close} notify={notify} report={report} />}
      {m?.kind === "notes" && (
        <NotesWindow
          notes={notes}
          canEdit={can.notes}
          onClose={close}
          onSave={(patch) => (Object.keys(patch).length ? run({ id: NOTES_CMD, params: patch }).catch((e: unknown) => (report(e), Promise.reject(e))) : Promise.resolve())}
        />
      )}
      {m?.kind === "trace" && traceImage && (
        <TraceDialog
          image={traceImage}
          run={run}
          report={report}
          onClose={close}
          onDone={(ids) => {
            close();
            if (ids.length) p.selectIds(ids);
            notify(t("files.traced"));
          }}
        />
      )}
      {m?.kind === "toBitmap" && doc && (
        <ConvertBitmapDialog
          objects={doc.objects.filter((o) => m.ids.includes(o.id))}
          run={run}
          report={report}
          onClose={close}
          onDone={(ids) => {
            close();
            if (ids.length) p.selectIds(ids);
          }}
        />
      )}
      {m?.kind === "adjust" && adjustLayer && doc && (
        <AdjustImageDialog
          layer={adjustLayer}
          images={doc.objects.filter((o) => m.ids.includes(o.id))}
          canApply={can.adjust}
          onClose={close}
          onApply={(ids, a, image) => {
            close();
            // A preset that holds the layer's image settings applies them too (one undo step).
            run(image ? [imageAdjust(ids, a), imageUpdate(m.layer, image)] : imageAdjust(ids, a)).catch(report);
          }}
        />
      )}
      {m?.kind === "editModel" && (
        <EditModelDialog
          name={m.name}
          initial={m.options}
          busy={busy}
          onClose={close}
          onApply={(o) => {
            setBusy(true);
            run(editModelInvocation(m.id, o))
              .then(() => {
                close();
                notify(t("files.modelEdited", { name: m.name }));
              })
              .catch(report)
              .finally(() => setBusy(false));
          }}
        />
      )}
      {m?.kind === "confirm" && (
        <Modal
          help="files-and-projects#start-a-new-document"
          title={t("files.unsavedTitle")}
          onClose={close}
          footer={
            <>
              <button className="btn" onClick={close}>
                {t("common.cancel")}
              </button>
              <button
                className="btn btn-warn"
                onClick={() => {
                  setModal(null);
                  m.action();
                }}
              >
                {m.label}
              </button>
            </>
          }
        >
          <p>{m.message}</p>
        </Modal>
      )}
    </>
  );

  const pagesDock = useMemo(() => (p.design ? <PagesDock doc={doc} has={has} run={run} report={report} /> : null), [p.design, doc, has, run, report]);

  const openTemplates = () => setModal({ kind: "templates" });
  return { menu, contextItems, handlers, dialogs, pagesDock, intercept, openOther, modalOpen: modal !== null, openTemplates };
}

/** File > Recent templates (this browser's list, kept to the templates that still exist). */
function RecentTemplates(p: { disabled: boolean; onOpen: (name: string) => void }) {
  const t = useT();
  const recent = useFilesStore().recentTemplates;
  const [list, setList] = useState<TemplateEntry[] | null>(null);
  useEffect(() => {
    if (recent.length === 0) return;
    let alive = true;
    filesApi
      .templates()
      .then((r) => alive && setList(liveRecent(recent, templateList(r))))
      .catch(() => alive && setList([]));
    return () => {
      alive = false;
    };
  }, [recent]);
  if (recent.length === 0 || !list || list.length === 0) return null;
  return (
    <div role="group" aria-label={t("files.recentTemplates")}>
      <div className="menu-label">{t("files.recentTemplates")}</div>
      {list.map((tp) => (
        <button key={tp.name} role="menuitem" className="menu-item recent-item" disabled={p.disabled} onClick={() => p.onOpen(tp.name)}>
          <span className="recent-name">{tp.name}</span>
          {tp.kind === "lbt" && <span className="menu-hint">.lbt</span>}
        </button>
      ))}
    </div>
  );
}

/** "Autosave every 5 min · last 3 min ago" under File > Restore. */
function AutosaveStatus(p: { project: string | null }) {
  const t = useT();
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void Promise.all([filesApi.fileSettings().then(fileSettingsOf), filesApi.backups(p.project).then(backupLists)])
      .then(([s, b]) => {
        if (!alive) return;
        const st = autosaveStatus(s, b.autosaves[0] ?? null, Date.now());
        setText(
          !st.on
            ? t("files.autosaveIsOff")
            : st.agoMin === null
              ? t("files.autosaveEveryNone", { n: st.minutes })
              : t("files.autosaveEveryAgo", { n: st.minutes, ago: st.agoMin }),
        );
      })
      .catch(() => alive && setText(null));
    return () => {
      alive = false;
    };
  }, [p.project, t]);
  if (!text) return null;
  return (
    <div className="menu-item muted files-autosave" role="note">
      {text}
    </div>
  );
}
