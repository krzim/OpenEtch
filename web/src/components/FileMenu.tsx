// The File menu, Fusion-style at the far left of the top bar: New, Open (engine projects or a file from this device),
// Save / Save as (engine projects), Import LightBurn (.lbrn2, replace or merge), Export to LightBurn and Download
// .openetch. `useFileActions` holds the state and actions; `FileMenu` is the tap-to-open dropdown and `FileDialogs`
// the dialogs and the dismissible warnings panel. Downloads are fetched with the Authorization header and saved from
// a blob link, so they work with token auth. Replacing a document with unsaved changes asks first.

import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { api, execute } from "../api/client.ts";
import * as C from "../api/commands.ts";
import {
  MAX_UPLOAD_BYTES,
  commandResult,
  currentProjectName,
  downloadName,
  fieldChoices,
  fileFormat,
  fileStem,
  formatBytes,
  formatModified,
  groupWarnings,
  importMessage,
  importOutcome,
  needsConfirm,
  projectDirty,
  projectList,
  sameField,
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

/** Which File dialog is open. `back` is where Cancel on a confirmation returns to. */
type FileModal =
  | { kind: "open" }
  | { kind: "saveAs" }
  | { kind: "import"; file: File | null; mode: ImportMode }
  | { kind: "confirm"; title: string; message: string; confirmLabel: string; action: () => void; back: FileModal | null };

export interface FileActions {
  /** The project the document is saved as, or null for an unsaved document. */
  projectName: string | null;
  /** Unsaved changes; null when the engine reports no project state. */
  dirty: boolean | null;
  busy: boolean;
  modal: FileModal | null;
  warnings: { title: string; items: string[] } | null;
  /** Name offered by Save as. */
  suggestedName: string;
  /** Field size the LightBurn import defaults to. */
  defaultFieldMm: number;
  docFieldMm: number | null;
  machineFieldMm: number | null;
  setModal: (m: FileModal | null) => void;
  dismissWarnings: () => void;
  newDocument: () => void;
  save: () => void;
  saveAs: (name: string) => Promise<boolean>;
  openProject: (name: string) => void;
  uploadFile: (file: File) => void;
  importLbrn2: (file: File, mode: ImportMode, fieldMm: number) => void;
  exportLbrn2: () => void;
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

export function useFileActions(p: { engine: Engine; machineFieldMm: number | null; fallbackFieldMm: number; notify: Notify; report: (e: unknown) => void }): FileActions {
  const { engine, notify, report } = p;
  const { doc, project } = engine;
  const [busy, setBusy] = useState(false);
  const [modal, setModal] = useState<FileModal | null>(null);
  const [warnings, setWarnings] = useState<{ title: string; items: string[] } | null>(null);
  const [suggestedName, setSuggestedName] = useState("");
  // The engine's project name when this UI last replaced the document (see `currentProjectName`); undefined when the
  // document is the project the engine reports.
  const [detachedFrom, setDetachedFrom] = useState<string | null | undefined>(undefined);

  const projectName = currentProjectName(project, detachedFrom);
  const dirty = projectDirty(project, doc?.revision ?? null);
  const docFieldMm = validField(doc?.field_mm);
  const defaultFieldMm = docFieldMm ?? p.machineFieldMm ?? p.fallbackFieldMm;

  // Once the engine reports another project (opened or saved elsewhere, or cleared), the document follows it again.
  useEffect(() => {
    if (detachedFrom !== undefined && (project?.name ?? null) !== detachedFrom) setDetachedFrom(undefined);
  }, [project?.name, detachedFrom]);

  // Latest values for callbacks that run after a confirmation.
  const live = useRef({ project, doc, dirty, projectName });
  live.current = { project, doc, dirty, projectName };

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
        notify("New document.");
      });
    }, null);
  }, [guard, task, engine, detach, notify]);

  const saveAs = useCallback(
    async (name: string): Promise<boolean> => {
      const problem = validateProjectName(name);
      if (problem) {
        notify(problem, "error");
        return false;
      }
      setBusy(true);
      try {
        await api.saveProject(name);
        setDetachedFrom(undefined);
        setSuggestedName("");
        notify(`Saved "${name}".`);
        return true;
      } catch (e) {
        report(e);
        return false;
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
          notify(`Opened "${name}".`);
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, notify],
  );

  const uploadFile = useCallback(
    (file: File) => {
      const format = fileFormat(file.name);
      if (format === null) {
        notify("Choose a .openetch project or a LightBurn .lbrn2 file.", "error");
        return;
      }
      if (format === "lbrn2") {
        // LightBurn files need a field size and a replace/merge choice.
        setModal({ kind: "import", file, mode: "replace" });
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        notify(`${file.name} is larger than ${formatBytes(MAX_UPLOAD_BYTES)}.`, "error");
        return;
      }
      guard(`Opening ${file.name}`, "Discard and open", () => {
        setModal(null);
        void task(async () => {
          const result = commandResult(await api.upload(file, "openetch"));
          detach(suggestProjectName(fileStem(file.name)));
          notify(`Opened ${file.name}. Save it to keep it on the engine.`);
          showWarnings(`Opening ${file.name}`, warningList(result));
          await engine.refreshDoc();
        });
      }, { kind: "open" });
    },
    [guard, task, engine, detach, notify, showWarnings],
  );

  const importLbrn2 = useCallback(
    (file: File, mode: ImportMode, fieldMm: number) => {
      const go = () => {
        setModal(null);
        void task(async () => {
          // The upload route takes the raw file (up to 50 MB) and the field size it was laid out for.
          if (file.size > MAX_UPLOAD_BYTES) throw new Error(`${file.name} is larger than ${formatBytes(MAX_UPLOAD_BYTES)}.`);
          const field = sameField(fieldMm, validField(live.current.doc?.field_mm)) ? undefined : fieldMm;
          const result = commandResult(await api.upload(file, "lbrn2", mode, field));
          await engine.refreshDoc();
          const outcome = importOutcome(result);
          if (mode === "replace") detach(suggestProjectName(fileStem(file.name)));
          notify(importMessage(outcome, mode));
          showWarnings(`Import of ${file.name}`, outcome.warnings);
        });
      };
      if (mode === "replace") guard(`Importing ${file.name}`, "Discard and import", go, { kind: "import", file, mode });
      else go();
    },
    [guard, task, engine, detach, notify, showWarnings],
  );

  const baseName = () => live.current.projectName ?? live.current.doc?.name ?? null;

  const exportLbrn2 = useCallback(() => {
    void task(async () => {
      // The document's field size, or the machine's when the document has none.
      const field = validField(live.current.doc?.field_mm) === null ? (p.machineFieldMm ?? p.fallbackFieldMm) : undefined;
      const res = await execute(C.exportLbrn2(field));
      const r = res.result as { content?: unknown } | null;
      if (!r || typeof r.content !== "string") throw new Error("the engine returned no LightBurn content");
      const name = downloadName(baseName(), "lbrn2");
      saveBlob(new Blob([r.content], { type: "application/xml" }), name);
      const w = warningList(r);
      notify(`Exported ${name}.${w.length ? ` ${w.length} warning${w.length === 1 ? "" : "s"}.` : ""}`);
      showWarnings(`Export of ${name}`, w);
    });
  }, [task, notify, showWarnings, p.machineFieldMm, p.fallbackFieldMm]);

  const downloadOpenetch = useCallback(() => {
    void task(async () => {
      const { blob, filename } = await api.download("openetch");
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
    modal,
    warnings,
    suggestedName,
    defaultFieldMm,
    docFieldMm,
    machineFieldMm: p.machineFieldMm,
    setModal,
    dismissWarnings: () => setWarnings(null),
    newDocument,
    save,
    saveAs,
    openProject,
    uploadFile,
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

/** The File button and its tap-to-open dropdown. */
export function FileMenu({ file }: { file: FileActions }) {
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
          {item("Open…", () => file.setModal({ kind: "open" }))}
          {item("Save", file.save, saveShortcut())}
          {item("Save as…", () => file.setModal({ kind: "saveAs" }))}
          <div className="menu-sep" role="separator" />
          {item("Import LightBurn…", () => file.setModal({ kind: "import", file: null, mode: "replace" }), ".lbrn2")}
          {item("Export to LightBurn", file.exportLbrn2, ".lbrn2")}
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
      {m?.kind === "import" && <ImportDialog file={file} initialFile={m.file} initialMode={m.mode} onClose={close} />}
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
          if (f) file.uploadFile(f);
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
  const { list } = useProjectList();
  const trimmed = name.trim();
  const problem = trimmed ? validateProjectName(trimmed) : null;
  const exists = !!list?.some((p) => p.name === trimmed) && trimmed !== file.projectName;
  const submit = async () => {
    if (!trimmed || problem || file.busy) return;
    if (exists && !confirmReplace) {
      setConfirmReplace(true);
      return;
    }
    if (await file.saveAs(trimmed)) onClose();
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
      {exists && <p className="hint warn-text">A project named "{trimmed}" exists. Saving replaces it.</p>}
      <p className="hint">Saved on the engine as {trimmed && !problem ? `"${trimmed}.openetch"` : "<name>.openetch"}.</p>
    </Modal>
  );
}

function ImportDialog(p: { file: FileActions; initialFile: File | null; initialMode: ImportMode; onClose: () => void }) {
  const { file } = p;
  const [picked, setPicked] = useState<File | null>(p.initialFile);
  const [mode, setMode] = useState<ImportMode>(p.initialMode);
  const [field, setField] = useState(fmt(file.defaultFieldMm, 3));
  const input = useRef<HTMLInputElement>(null);
  const n = Number(field.trim());
  const fieldOk = field.trim() !== "" && Number.isFinite(n) && n > 0 && n <= 2000;
  const error = !fieldOk ? "Field size must be a number between 0 and 2000 mm." : null;
  const choices = fieldChoices(file.docFieldMm, file.machineFieldMm);
  return (
    <Modal
      title="Import LightBurn project"
      onClose={p.onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!picked || !fieldOk || file.busy} onClick={() => picked && file.importLbrn2(picked, mode, n)}>
            Import
          </button>
        </>
      }
    >
      <input
        ref={input}
        type="file"
        accept=".lbrn2"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = "";
          if (f) setPicked(f);
        }}
      />
      <div className="field">
        <span className="field-label">File</span>
        <button className="btn file-pick" onClick={() => input.current?.click()}>
          <Icon name="upload" />
          <span className="file-pick-name">{picked ? `${picked.name} (${formatBytes(picked.size)})` : "Choose a .lbrn2 file…"}</span>
        </button>
      </div>
      <div className="segmented">
        <button className={mode === "replace" ? "active" : ""} onClick={() => setMode("replace")}>
          Replace document
        </button>
        <button className={mode === "merge" ? "active" : ""} onClick={() => setMode("merge")}>
          Merge into document
        </button>
      </div>
      <label className="field">
        <span className="field-label">Field size</span>
        <span className="field-input">
          <input type="text" inputMode="decimal" value={field} onChange={(e) => setField(e.target.value)} />
          <span className="unit">mm</span>
        </span>
      </label>
      <div className="chip-row">
        {choices.map((c) => (
          <button key={c} className={`chip${fieldOk && Math.abs(c - n) < 1e-9 ? " active" : ""}`} onClick={() => setField(fmt(c, 3))}>
            {fmt(c, 3)} mm
            {c === file.docFieldMm ? " (document)" : c === file.machineFieldMm ? " (machine)" : ""}
          </button>
        ))}
      </div>
      <p className="hint">
        LightBurn files do not record the galvo field size. Use the size the project was laid out with in LightBurn; a
        different size shifts everything by half the difference. Text comes in as outlines; images are skipped.
      </p>
    </Modal>
  );
}
