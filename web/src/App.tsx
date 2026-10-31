// Application shell: top bar, toolbar, Browser | Viewport | Properties, Timeline. On narrow screens (iPad portrait,
// phones) the side panels become drawers over the viewport. All document state comes from the engine; the only
// client state is the view: workspace, tool, selection, open panels and dialogs.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "./api/client.ts";
import * as C from "./api/commands.ts";
import { pendingRuns, stopToast } from "./api/runs.ts";
import { useEngine } from "./api/useEngine.ts";
import { BrowserPanel } from "./components/BrowserPanel.tsx";
import { ArrayDialog, CommandPalette, PlaceDialog, StockDialog } from "./components/Dialogs.tsx";
import { FileDialogs, FileMenu, useFileActions } from "./components/FileMenu.tsx";
import { MachineCard, PreviewStatsCard, RunsCard } from "./components/LaserPanel.tsx";
import { LayerSection, PropertiesPanel } from "./components/PropertiesPanel.tsx";
import { Timeline } from "./components/Timeline.tsx";
import { Toolbar } from "./components/Toolbar.tsx";
import { TopBar, type Workspace } from "./components/TopBar.tsx";
import { Viewport, type Tool } from "./components/Viewport.tsx";
import { EMPTY_SELECTION, clickObject, pruneSelection, type Selection } from "./selection.ts";

/** Field size until /api/machine reports `field_mm`. */
const DEFAULT_FIELD_MM = 153.75;
/** Below this width the side panels are drawers. */
const NARROW_QUERY = "(max-width: 1023px)";

type Dialog = "stock" | "place" | "array" | "commands" | null;
interface Toast {
  id: number;
  msg: string;
  kind: "info" | "error";
}

function useMediaQuery(q: string): boolean {
  const [m, setM] = useState(() => window.matchMedia(q).matches);
  useEffect(() => {
    const mq = window.matchMedia(q);
    const on = () => setM(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, [q]);
  return m;
}

export function App() {
  const [workspace, setWorkspace] = useState<Workspace>("design");
  const [tool, setTool] = useState<Tool>("select");
  const [selection, setSelection] = useState<Selection>(EMPTY_SELECTION);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [showPreview, setShowPreview] = useState(true);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const narrow = useMediaQuery(NARROW_QUERY);
  const [leftOpen, setLeftOpen] = useState(!narrow);
  const [rightOpen, setRightOpen] = useState(!narrow);
  const fileRef = useRef<HTMLInputElement>(null);
  const toastId = useRef(0);

  const notify = useCallback((msg: string, kind: "info" | "error" = "info") => {
    const id = ++toastId.current;
    setToasts((t) => [...t.slice(-3), { id, msg, kind }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 7000 : 4000);
  }, []);
  const report = useCallback((e: unknown) => notify(e instanceof Error ? e.message : String(e), "error"), [notify]);

  const previewOn = workspace === "laser" && showPreview;
  const engine = useEngine({ wantPreview: previewOn, notify });
  const { doc, machine } = engine;

  // Drawers start closed when the layout becomes narrow, open when it becomes wide.
  useEffect(() => {
    setLeftOpen(!narrow);
    setRightOpen(!narrow);
  }, [narrow]);

  useEffect(() => setSelection((s) => pruneSelection(s, doc)), [doc]);

  const missingCommands = useMemo(() => {
    if (!engine.commands || engine.commands.length === 0) return [];
    const have = new Set(engine.commands.map((c) => c.id));
    return C.REQUIRED_COMMANDS.filter((id) => !have.has(id));
  }, [engine.commands]);

  const machineFieldMm = typeof machine?.field_mm === "number" && machine.field_mm > 0 ? machine.field_mm : null;
  const fieldMm = machineFieldMm ?? DEFAULT_FIELD_MM;
  const file = useFileActions({ engine, machineFieldMm, fallbackFieldMm: DEFAULT_FIELD_MM, notify, report });
  const fileModalOpen = file.modal !== null;
  const saveFile = file.save;
  const activeLayer = selection.layer ?? doc?.layers[0]?.id ?? null;
  const runs = pendingRuns(machine);

  const run = engine.run;
  const exec = useCallback((inv: Parameters<typeof run>[0]) => run(inv).catch(report), [run, report]);

  const selectObject = useCallback(
    (id: string, additive: boolean) => {
      if (!doc) return;
      setSelection((s) => clickObject(s, doc, id, additive));
      if (narrow && workspace === "design") setRightOpen(true);
    },
    [doc, narrow, workspace],
  );

  const deleteSelection = useCallback(() => {
    if (selection.objects.length) void exec(C.remove(selection.objects));
  }, [selection.objects, exec]);

  const undo = useCallback(() => engine.undo().catch(report), [engine, report]);
  const redo = useCallback(() => engine.redo().catch(report), [engine, report]);

  // Emergency stop: aborts any run or frame and cancels pending runs. Framing alone ends with "Stop frame".
  const stop = useCallback(() => {
    api
      .stop()
      .then((res) => notify(stopToast(res)))
      .catch(report)
      .finally(() => void engine.refreshMachine());
  }, [engine, notify, report]);

  // Keyboard shortcuts (ignored while typing in a field, except Ctrl/Cmd+S).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "s") {
        // Never the browser's "save page"; Save prompts for a name the first time.
        e.preventDefault();
        if (dialog || fileModalOpen || e.repeat) return;
        // Commit a field being edited (its value is sent on blur) before saving.
        if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
        saveFile();
        return;
      }
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      if (dialog || fileModalOpen) return;
      if (mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) void redo();
        else void undo();
      } else if (mod && e.key.toLowerCase() === "y") {
        e.preventDefault();
        void redo();
      } else if (e.key === "Escape") {
        if (tool !== "select") setTool("select");
        else setSelection(EMPTY_SELECTION);
      } else if ((e.key === "Delete" || e.key === "Backspace") && workspace === "design") {
        e.preventDefault();
        deleteSelection();
      } else if (!mod && workspace === "design" && (e.key === "r" || e.key === "R")) {
        setTool("rect");
      } else if (!mod && workspace === "design" && (e.key === "e" || e.key === "E")) {
        setTool("ellipse");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dialog, fileModalOpen, saveFile, tool, workspace, undo, redo, deleteSelection]);

  const onImportFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    try {
      const text = await file.text();
      await run(C.importSvg(text, file.name.replace(/\.svg$/i, ""), activeLayer));
      notify(`Imported ${file.name}.`);
    } catch (err) {
      report(err);
    }
  };

  const machineAction = (p: Promise<unknown>, ok: string) =>
    p
      .then(() => notify(ok))
      .catch(report)
      .finally(() => void engine.refreshMachine());

  const switchWorkspace = (w: Workspace) => {
    setWorkspace(w);
    setTool("select");
  };

  const closeDialog = useCallback(() => setDialog(null), []);
  const selectedLayer = selection.layer ? doc?.layers.find((l) => l.id === selection.layer) : undefined;

  return (
    <div className={`app ws-${workspace}${narrow ? " narrow" : ""}`}>
      <TopBar
        fileMenu={<FileMenu file={file} />}
        workspace={workspace}
        onWorkspace={switchWorkspace}
        onUndo={() => void undo()}
        onRedo={() => void redo()}
        onStop={stop}
        link={engine.link}
        httpError={engine.httpError}
        machine={machine}
        missingCommands={missingCommands}
        docName={doc?.name ?? null}
        projectName={file.projectName}
        dirty={file.dirty}
        pendingRuns={runs.length}
      />
      <Toolbar
        workspace={workspace}
        tool={tool}
        hasSelection={selection.objects.length > 0}
        hasStock={!!doc?.stock}
        showPreview={showPreview}
        leftOpen={leftOpen}
        rightOpen={rightOpen}
        onTool={setTool}
        onStock={() => setDialog("stock")}
        onImport={() => fileRef.current?.click()}
        onPlace={() => setDialog("place")}
        onArray={() => setDialog("array")}
        onDelete={deleteSelection}
        onCommands={() => setDialog("commands")}
        onTogglePreview={() => setShowPreview((v) => !v)}
        onFrame={() => void machineAction(api.frame(), "Framing with the red pointer.")}
        onFrameStop={() => void machineAction(api.frameStop(), "Framing stopped. Pending runs are kept.")}
        onRequestRun={() => void machineAction(api.requestRun(), "Run requested. Approve it in the Machine panel.")}
        onToggleLeft={() => setLeftOpen((v) => !v)}
        onToggleRight={() => setRightOpen((v) => !v)}
      />
      <div className="main">
        {narrow && (leftOpen || rightOpen) && (
          <div
            className="drawer-scrim"
            onClick={() => {
              setLeftOpen(false);
              setRightOpen(false);
            }}
          />
        )}
        <aside className={`panel panel-left${leftOpen ? " open" : ""}`} aria-label="Browser">
          <div className="panel-title">Browser</div>
          <div className="panel-body">
            <BrowserPanel
              doc={doc}
              selection={selection}
              onSelectObject={selectObject}
              onSelectLayer={(id) => setSelection({ objects: [], layer: id, stock: false })}
              onSelectStock={() => setSelection({ objects: [], layer: null, stock: true })}
              run={run}
              report={report}
            />
          </div>
        </aside>

        <section className="center">
          <Viewport
            doc={doc}
            fieldMm={fieldMm}
            selection={selection}
            tool={tool}
            activeLayer={activeLayer}
            preview={engine.preview}
            showPreview={previewOn}
            onSelectObject={selectObject}
            onClearSelection={() => setSelection(EMPTY_SELECTION)}
            onToolDone={() => setTool("select")}
            run={run}
            notify={notify}
          />
        </section>

        <aside className={`panel panel-right${rightOpen ? " open" : ""}`} aria-label={workspace === "laser" ? "Machine" : "Properties"}>
          <div className="panel-title">{workspace === "laser" ? "Machine" : "Properties"}</div>
          <div className="panel-body">
            {workspace === "design" ? (
              <PropertiesPanel doc={doc} selection={selection} run={run} report={report} onEditStock={() => setDialog("stock")} />
            ) : (
              <div className="props">
                <MachineCard machine={machine} httpError={engine.httpError} />
                <RunsCard
                  machine={machine}
                  docRevision={doc?.revision ?? null}
                  onChanged={() => void engine.refreshMachine()}
                  report={report}
                />
                <PreviewStatsCard preview={engine.preview} enabled={previewOn} />
                {selectedLayer && <LayerSection layer={selectedLayer} run={run} report={report} />}
              </div>
            )}
          </div>
        </aside>
      </div>
      <Timeline history={engine.history} revision={doc?.revision ?? null} flash={engine.remoteEdit} />

      <input ref={fileRef} type="file" accept=".svg,image/svg+xml" hidden onChange={onImportFile} />

      {dialog === "stock" && <StockDialog doc={doc} run={run} onClose={closeDialog} />}
      {dialog === "place" && <PlaceDialog ids={selection.objects} run={run} onClose={closeDialog} />}
      {dialog === "array" && doc && <ArrayDialog doc={doc} ids={selection.objects} run={run} onClose={closeDialog} />}
      {dialog === "commands" && <CommandPalette commands={engine.commands} run={run} onClose={closeDialog} />}
      <FileDialogs file={file} docName={doc?.name ?? null} />

      <div className="toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.kind}`} onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))}>
            {t.msg}
          </div>
        ))}
      </div>
    </div>
  );
}
