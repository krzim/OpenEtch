// Application shell: top bar, toolbar, Browser | Viewport | Properties, Timeline. On narrow screens (iPad portrait,
// phones) the side panels become drawers over the viewport. All document state comes from the engine; the only
// client state is the view: workspace, tool, selection, clipboard, open panels and dialogs, display options.
//
// Files dropped anywhere on the window (or picked with Import / Ctrl+I) are routed by api/drop.ts: SVG, DXF and bitmaps
// are imported into the document, a LightBurn .lbrn2 opens the Open/Import dialog, a .openetch opens the project.
//
// The canvas shows the active device's workspace (components/Devices.tsx): a galvo's square field or a gantry
// device's rectangle. File > Devices… (Alt+Shift+M, as in LightBurn) opens the Devices window, and the Laser workspace
// has the device dropdown.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "./api/client.ts";
import * as C from "./api/commands.ts";
import {
  importAccept,
  bytesToBase64,
  createdIds,
  dropSummary,
  importName,
  inlineLimit,
  planIsEmpty,
  routeDroppedFiles,
  type DirectImport,
} from "./api/drop.ts";
import { friendlyError, outsideField } from "./api/laser.ts";
import { activeDevice, asLightBurnDevices, canvasWorkspace, designOnlyReason, machineDevice } from "./api/devices.ts";
import { engineLightBurnDevice } from "./api/lightburn.ts";
import { MAX_UPLOAD_BYTES, commandResult, defaultImportMode, fileError, warningList } from "./api/project.ts";
import { noPinMode, pendingRuns, stopToast } from "./api/runs.ts";
import type { Vec2 } from "./api/types.ts";
import { useEngine } from "./api/useEngine.ts";
import { BrowserPanel } from "./components/BrowserPanel.tsx";
import { ContextMenu, type MenuItem } from "./components/ContextMenu.tsx";
import { CommandPalette, PlaceDialog, StockDialog } from "./components/Dialogs.tsx";
import { DeviceBar, DevicesWindow, useDevices } from "./components/Devices.tsx";
import { useEditTools } from "./components/EditTools.tsx";
import { useTextTools } from "./text/useTextTools.tsx";
import { guideScope } from "./guides.ts";
import { DropOverlay, useFileDrop } from "./components/FileDrop.tsx";
import { FileDialogs, FileMenu, useFileActions } from "./components/FileMenu.tsx";
import { ENGINE_ACCEPT, importTargetLayer } from "./files/fileFormats.ts";
import { filesState } from "./files/filesStore.ts";
import { useFilesUi } from "./files/useFilesUi.tsx";
import { ShutdownBadge, SimPinBadge } from "./libs/JobSafety.tsx";
import { LaserExtras } from "./libs/LaserExtras.tsx";
import { useLibsUi } from "./libs/useLibsUi.tsx";
import { useDesignUi } from "./design/useDesignUi.tsx";
import { MachineCard, PreviewStatsCard, RunsCard } from "./components/LaserPanel.tsx";
import { frameWithOptions } from "./components/FramingOptions.tsx";
import { runJob, togglePause } from "./components/JobControls.tsx";
import { PreviewTimeline } from "./components/PreviewTimeline.tsx";
import { LightBurnDevices, useLightBurn } from "./components/LightBurn.tsx";
import { LayerSection, PropertiesPanel } from "./components/PropertiesPanel.tsx";
import { Timeline } from "./components/Timeline.tsx";
import { Toolbar, type ArrangeAction } from "./components/Toolbar.tsx";
import { TopBar, type Workspace } from "./components/TopBar.tsx";
import { Viewport, type PreviewOptions, type SnapSettings, type Tool, type ViewStyle } from "./components/Viewport.tsx";
import { CutsLayers } from "./workspace/CutsLayers.tsx";
import { TopActions } from "./workspace/HelpMenu.tsx";
import { LaserDock, machineState } from "./workspace/LaserDock.tsx";
import { NumericBar } from "./workspace/NumericBar.tsx";
import { PaletteBar } from "./workspace/PaletteBar.tsx";
import { getPrefs } from "./workspace/prefs.ts";
import { hintFor } from "./workspace/hotkeys.ts";
import { VIEW_PREFS_EVENT } from "./files/LightBurnPrefsImport.tsx";
import { RecentProjects } from "./workspace/RecentProjects.tsx";
import { StatusBar } from "./workspace/StatusBar.tsx";
import { sendViewCommand } from "./workspace/viewStatus.ts";
import { WorkspaceFileItems, useWorkspaceUi } from "./workspace/useWorkspace.tsx";
import { Dock } from "./workspace/Dock.tsx";
import { DockAreaHost, FloatHost, WindowMenu, useAreaHasWindows } from "./workspace/DockHost.tsx";
import { useDocTabs } from "./workspace/DocTabs.tsx";
import { useDocking } from "./workspace/DockingTool.tsx";
import { useFlash, withLayerFlash } from "./workspace/flash.tsx";
import { togglePositionLaser, usePositionLaser, withPositionLaser } from "./workspace/positionLaser.tsx";
import { requestRotarySetup } from "./workspace/LaserDock.tsx";
import { MoveTogetherDialog } from "./design/DesignDialogs.tsx";
import { jobApi, saveBlob } from "./api/jobApi.ts";
import { gcodeFileName, isGrblMachine } from "./api/grbl.ts";
import { GrblConsole } from "./components/GrblConsole.tsx";
import { GrblMove } from "./components/GrblMove.tsx";
import { usePrefs, useT } from "./workspace/usePrefs.ts";
import { alignMoves, distributeMoves, nudgeDelta, nudgeKeyId, queueNudge, rotateKeyDeg, roundMm, type AlignX, type AlignY } from "./editing.ts";
import { boxCenter, objectBounds, unionBoxes, type Box } from "./geometry.ts";
import {
  EMPTY_SELECTION,
  alignReference,
  anyGrouped,
  applySelect,
  cycleSelection,
  invertSelection,
  pruneSelection,
  selectAll,
  selectLayerObjects,
  topGroup,
  unitMembers,
  unitsOf,
  type SelectOp,
  type Selection,
} from "./selection.ts";

/** Field size until /api/machine reports `field_mm`. */
const DEFAULT_FIELD_MM = 153.75;
/** Below this width the side panels are drawers. */
const NARROW_QUERY = "(max-width: 1023px)";
const VIEW_KEY = "openetch.view";

type Dialog = "stock" | "place" | "commands" | null;
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

interface ViewPrefs {
  snap: SnapSettings;
  viewStyle: ViewStyle;
}

const DEFAULT_PREFS: ViewPrefs = { snap: { objects: true, grid: false, gridMm: 1 }, viewStyle: "filled" };

function loadPrefs(): ViewPrefs {
  try {
    const raw = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "null") as Partial<ViewPrefs> | null;
    if (!raw || typeof raw !== "object") return DEFAULT_PREFS;
    const snap = { ...DEFAULT_PREFS.snap, ...(raw.snap ?? {}) };
    if (!(snap.gridMm > 0)) snap.gridMm = 1;
    return { snap, viewStyle: raw.viewStyle === "wireframe" ? "wireframe" : "filled" };
  } catch {
    return DEFAULT_PREFS;
  }
}

/** Typing in a field (or a dialog button) keeps its keys. */
function typingIn(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/** Whether the canvas (or nothing in particular) has keyboard focus, for LightBurn's canvas-only keys. */
function canvasFocused(): boolean {
  const a = document.activeElement;
  return !a || a === document.body || !!a.closest?.(".viewport");
}

export function App() {
  const [workspace, setWorkspace] = useState<Workspace>("design");
  const [tool, setTool] = useState<Tool>("select");
  const [selection, setSelectionRaw] = useState<Selection>(EMPTY_SELECTION);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [showPreview, setShowPreview] = useState(true);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [prefs, setPrefs] = useState<ViewPrefs>(loadPrefs);
  const [previewOptions, setPreviewOptions] = useState<PreviewOptions>({ hidden: new Set(), hiddenPasses: new Set(), upTo: null, shade: true, jumps: true });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const narrow = useMediaQuery(NARROW_QUERY);
  const [leftOpen, setLeftOpen] = useState(!narrow);
  const [rightOpen, setRightOpen] = useState(!narrow);
  const fileRef = useRef<HTMLInputElement>(null);
  const toastId = useRef(0);
  const cursor = useRef<Vec2 | null>(null);
  const importing = useRef(false);
  const openOtherRef = useRef<(f: File) => void>(() => undefined);

  // The LightBurn preferences import (files/LightBurnPrefsImport.tsx) sets the canvas snapping and view style.
  useEffect(() => {
    const on = (e: Event) => {
      const v = (e as CustomEvent<{ snap?: Partial<SnapSettings>; viewStyle?: ViewStyle }>).detail;
      setPrefs((p) => ({ snap: { ...p.snap, ...(v?.snap ?? {}) }, viewStyle: v?.viewStyle ?? p.viewStyle }));
    };
    window.addEventListener(VIEW_PREFS_EVENT, on);
    return () => window.removeEventListener(VIEW_PREFS_EVENT, on);
  }, []);
  useEffect(() => {
    try {
      localStorage.setItem(VIEW_KEY, JSON.stringify(prefs));
    } catch {
      // Storage unavailable (private mode): the preference lasts for this visit.
    }
  }, [prefs]);

  const notify = useCallback((msg: string, kind: "info" | "error" = "info") => {
    const id = ++toastId.current;
    setToasts((t) => [...t.slice(-3), { id, msg, kind }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 7000 : 4000);
  }, []);

  const previewOn = workspace === "laser" && showPreview;
  const engine = useEngine({ wantPreview: previewOn, notify });
  const { doc, machine, has } = engine;
  const docRef = useRef(doc);
  docRef.current = doc;

  // Engine errors name objects and layers rather than internal ids, without the HTTP prefix.
  const report = useCallback((e: unknown) => notify(friendlyError(e instanceof Error ? e.message : String(e), docRef.current), "error"), [notify]);

  // Drawers start closed when the layout becomes narrow, open when it becomes wide.
  useEffect(() => {
    setLeftOpen(!narrow);
    setRightOpen(!narrow);
  }, [narrow]);

  useEffect(() => setSelectionRaw((s) => pruneSelection(s, doc)), [doc]);

  const setSelection = useCallback(
    (s: Selection) => {
      setSelectionRaw(s);
      if (narrow && workspace === "design" && (s.objects.length > 0 || s.stock)) setRightOpen(true);
    },
    [narrow, workspace],
  );
  const selectIds = useCallback((ids: string[]) => {
    const d = docRef.current;
    const layer = d?.objects.find((o) => o.id === ids[ids.length - 1])?.layer ?? null;
    setSelectionRaw({ objects: ids, layer, stock: false, lastAdded: ids, reference: null });
  }, []);

  const missingCommands = useMemo(() => {
    if (!engine.commands || engine.commands.length === 0) return [];
    const have = new Set(engine.commands.map((c) => c.id));
    return C.REQUIRED_COMMANDS.filter((id) => !have.has(id));
  }, [engine.commands]);

  const machineFieldMm = typeof machine?.field_mm === "number" && machine.field_mm > 0 ? machine.field_mm : null;
  const lightburn = useLightBurn(engine.link);
  const devices = useDevices(engine.link, engine.deviceSeq);
  // The active device: from the registry, else the one GET /api/machine reports; null on older engines.
  const device = activeDevice(devices.list) ?? machineDevice(machine);
  const [wsW, wsH] = canvasWorkspace(device, machineFieldMm, DEFAULT_FIELD_MM);
  const workspaceMm = useMemo<Vec2>(() => [wsW, wsH], [wsW, wsH]);
  // The LightBurn dialogs match files against the device registry (the active device is the engine's own); engines
  // without it fall back to LightBurn's devices.
  const lightburnDevices = useMemo(
    () =>
      devices.list
        ? { devices: asLightBurnDevices(devices.list.devices), engineDevice: device?.display_name ?? null, engineKind: "active-device" as const, active: device }
        : { devices: lightburn.info?.devices ?? [], engineDevice: engineLightBurnDevice(machine, lightburn.info) },
    [devices.list, device, lightburn.info, machine],
  );
  const file = useFileActions({
    engine,
    machineFieldMm,
    fallbackFieldMm: DEFAULT_FIELD_MM,
    notify,
    report,
    onImported: selectIds,
    lightburn: lightburnDevices,
    openOther: (f) => openOtherRef.current(f),
  });
  const fileModalOpen = file.modal !== null;
  const saveFile = file.save;
  const activeLayer = selection.layer ?? doc?.layers[0]?.id ?? null;
  // Export, Print, further import formats, templates, notes, backups, pages and image tools (src/files/).
  const filesUi = useFilesUi({ engine, file, selection: selection.objects, selectIds, activeLayer, design: workspace === "design", notify, report });
  openOtherRef.current = filesUi.openOther;
  // Material library, defaults, test generators, laser tools, camera, job log, Help additions (src/libs/).
  const libsUi = useLibsUi({
    doc,
    machine,
    selection: selection.objects,
    activeLayer,
    design: workspace === "design",
    designOnly: designOnlyReason(device),
    run: engine.run,
    notify,
    report,
    selectIds,
    refreshMachine: () => void engine.refreshMachine(),
    onStartPointTool: () => setTool("select"),
  });
  // Shape Properties, warp / deform, tangent circle, rubber band, toolpath to path, selection queries, page positions,
  // grid array, curve tolerance, the Art Library and the clipboard (src/design/).
  const designUi = useDesignUi({
    doc,
    selection,
    selectIds,
    activeLayer,
    workspaceMm,
    design: workspace === "design",
    blocked: !!dialog || fileModalOpen || devicesOpen || filesUi.modalOpen || libsUi.modalOpen,
    has,
    run: engine.run,
    notify,
    report,
    cursor,
    refreshDoc: () => void engine.refreshDoc(),
  });
  // The key handler reads the design layer through a ref (a new object each render would re-install it every time).
  const designRef = useRef(designUi);
  designRef.current = designUi;
  const runs = pendingRuns(machine);
  const outside = useMemo(() => outsideField(doc, workspaceMm), [doc, workspaceMm]);

  const run = engine.run;
  const exec = useCallback((inv: Parameters<typeof run>[0]) => run(inv).catch(report), [run, report]);

  // Workspace layer (workspace/): theme, preferences, the rebindable key map (installed before EditTools' keys).
  const ws = useWorkspaceUi();
  const t = useT();
  // Document tabs (several open documents, LightBurn's New Window), the Docking tool, Flash layer, dockable windows.
  const tabs = useDocTabs({
    dirty: file.dirty,
    refresh: () => engine.refreshDoc(),
    onSwitched: () => {
      setSelectionRaw(EMPTY_SELECTION);
      setTool("select");
    },
    notify,
    report,
  });
  const docking = useDocking({ doc, selected: selection.objects, workspaceMm, run, report, notify });
  const flash = useFlash();
  const leftHasWindows = useAreaHasWindows("left");
  const [moveTogether, setMoveTogether] = useState<"x" | "y" | null>(null);
  const positioning = usePositionLaser();
  const rightHasWindows = useAreaHasWindows("right");

  // LightBurn's Arrange > Distribute (H-Spaced / H-Centered / V-Spaced / V-Centered), hidden in Beginner Mode like
  // LightBurn's. The arrange handler is defined further down; the menu reaches it through a ref.
  const { beginnerMode, hotkeys: prefHotkeys } = usePrefs();
  const arrangeRef = useRef<(a: ArrangeAction) => void>(() => {});
  const nUnits = doc ? unitsOf(doc, selection.objects).length : 0;
  const distributeItems: MenuItem[] = beginnerMode
    ? []
    : [
        { sep: true },
        { label: t("app.distHSpaced"), hint: hintFor("arrange.distHSpaced", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distH") },
        { label: t("app.distHCentered"), hint: hintFor("arrange.distHCentered", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distHc") },
        { label: t("app.distVSpaced"), hint: hintFor("arrange.distVSpaced", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distV") },
        { label: t("app.distVCentered"), hint: hintFor("arrange.distVCentered", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distVc") },
      ];

  // Node editing, line/pen/polygon, measure, trim, rulers and guides, the Modify and Arrange menus (EditTools.tsx).
  const edit = useEditTools({
    doc,
    selection,
    setSelection,
    selectIds,
    tool,
    setTool,
    activeLayer,
    workspaceMm,
    design: workspace === "design",
    blocked: !!dialog || fileModalOpen || devicesOpen || filesUi.modalOpen || libsUi.modalOpen || designUi.modalOpen,
    has,
    run,
    notify,
    report,
    onGridArray: designUi.openArray,
    arrangeExtra: [...distributeItems, ...designUi.arrangeItems],
    // Guides belong to the project (not the document's name, which projects can share); wait until both are known.
    guideScope: doc && engine.projectKnown ? guideScope(file.projectName, file.openedName) : null,
  });
  // Text and Barcode tools, text/barcode properties, the Variable Text window (text/useTextTools.tsx).
  const text = useTextTools({ doc, selection, selectIds, tool, setTool, activeLayer, design: workspace === "design", has, run, notify, report });

  // ---- import: drop and picker share one router ------------------------------------------------------------------

  const importOne = useCallback(
    async (imp: DirectImport<File>, layer: string | null): Promise<string[]> => {
      const name = importName(imp.file.name);
      if (imp.file.size > inlineLimit(imp.kind)) {
        // Large files go up raw; the engine imports them on its own thread.
        // Same layer and name as an inline import, so a big file lands on the active layer too.
        const res = await api
          .upload(imp.file, imp.kind, undefined, undefined, { layer, name })
          .catch((e: unknown) => Promise.reject(fileError(e, imp.file.name)));
        const result = commandResult(res);
        await engine.refreshDoc();
        return createdIds(result);
      }
      let inv;
      if (imp.kind === "image") {
        const bytes = new Uint8Array(await imp.file.arrayBuffer());
        inv = C.importImage(bytesToBase64(bytes), imp.mime ?? "image/png", name, layer);
      } else {
        const text = await imp.file.text();
        inv = imp.kind === "svg" ? C.importSvg(text, name, layer) : C.importDxf(text, name, layer);
      }
      const result = await run(inv);
      const w = warningList(result);
      if (w.length) notify(`${imp.file.name}: ${w.join("; ")}`);
      return createdIds(result);
    },
    [engine, run, notify],
  );

  const handleFiles = useCallback(
    async (files: File[]) => {
      const m = file.modal;
      // The Open dialog accepts a drop (it is about to open a file anyway); the LightBurn dialog takes .lbrn2 files.
      if (m?.kind === "import") {
        const lb = files.filter((f) => /\.lbrn2$/i.test(f.name));
        if (lb.length) file.setModal({ ...m, files: lb, nonce: (m.nonce ?? 0) + 1 });
        else notify(t("app.lbrnOnly"), "error");
        return;
      }
      // While another dialog is open or a file task runs, a drop is still kept from the browser but not imported.
      const blocked =
        importing.current || file.busy ? t("rest.importRunning") : dialog || filesUi.modalOpen || (m && m.kind !== "open") ? t("rest.finishDialog") : null;
      if (blocked) {
        notify(blocked, "error");
        return;
      }
      // PDF, AI, EPS, HPGL, Gerber, 3D models, ... (and SVG/DXF/bitmaps on engines that import them all) go through
      // the files layer, which applies the import options (src/files/useFilesUi.tsx).
      files = filesUi.intercept(files);
      if (files.length === 0) {
        if (m?.kind === "open") file.setModal(null);
        return;
      }
      const plan = routeDroppedFiles(files, { maxBytes: MAX_UPLOAD_BYTES, has });
      if (m?.kind === "open") file.setModal(null);
      if (plan.open) {
        file.uploadFile(plan.open);
        return;
      }
      if (planIsEmpty(plan)) {
        notify(dropSummary({ imported: [], objects: 0, refused: plan.refused }).msg, "error");
        return;
      }
      importing.current = true;
      file.setBusy(true);
      const imported: string[] = [];
      const created: string[] = [];
      const refused = [...plan.refused];
      try {
        for (const imp of plan.imports) {
          try {
            // Never onto a tool layer unless Import options allow it (LightBurn's "Allow Importing to Tool Layers").
            const ids = await importOne(imp, importTargetLayer(docRef.current?.layers, activeLayer, filesState().importPrefs.toolLayers));
            imported.push(imp.file.name);
            created.push(...ids);
          } catch (e) {
            refused.push({ name: imp.file.name, reason: friendlyError(e instanceof Error ? e.message : String(e), docRef.current) });
          }
        }
      } finally {
        importing.current = false;
        file.setBusy(false);
      }
      // LightBurn's "Select imported shapes": the new objects become the selection.
      if (created.length) {
        setWorkspace("design");
        selectIds(created);
      }
      // Open when the design is (still) empty, Import otherwise.
      if (plan.lbrn2.length) file.setModal({ kind: "import", files: plan.lbrn2, mode: created.length ? "merge" : defaultImportMode(docRef.current) });
      if (imported.length || refused.length) {
        const sum = dropSummary({ imported, objects: created.length, refused, pending: plan.lbrn2.length });
        notify(sum.msg, sum.kind);
      }
    },
    [file, dialog, has, notify, importOne, activeLayer, selectIds, filesUi],
  );

  const dropLabel = useFileDrop((files) => void handleFiles(files));
  const openImport = useCallback(() => fileRef.current?.click(), []);

  // ---- editing actions -------------------------------------------------------------------------------------------

  const boxOf = useCallback((ids: readonly string[]): Box | null => {
    const d = docRef.current;
    if (!d) return null;
    const set = new Set(ids);
    return unionBoxes(d.objects.filter((o) => set.has(o.id)).map(objectBounds));
  }, []);

  /** Units of the selection (groups as one) with their bounds. */
  const selUnits = useCallback(() => {
    const d = docRef.current;
    if (!d) return [];
    return unitsOf(d, selection.objects)
      .map((u) => ({ id: u, members: unitMembers(d, u), box: boxOf(unitMembers(d, u)) }))
      .filter((u): u is { id: string; members: string[]; box: Box } => u.box !== null);
  }, [selection.objects, boxOf]);

  /** Send per-unit moves as one undo step. */
  const sendMoves = useCallback(
    (moves: { id: string; d: Vec2 }[], units: { id: string; members: string[] }[]) => {
      const invs = moves
        .map((mv) => ({ ids: units.find((u) => u.id === mv.id)?.members ?? [], d: [roundMm(mv.d[0]), roundMm(mv.d[1])] as Vec2 }))
        .filter((mv) => mv.ids.length && (mv.d[0] !== 0 || mv.d[1] !== 0))
        .map((mv) => C.transform(mv.ids, { translate: mv.d }));
      if (invs.length) void exec(invs.length === 1 ? invs[0] : invs);
    },
    [exec],
  );

  const alignTo = useCallback(
    (x?: AlignX, y?: AlignY) => {
      const d = docRef.current;
      if (!d) return;
      const units = selUnits();
      if (units.length < 2) {
        notify(t("app.alignTwo"), "error");
        return;
      }
      const ref = alignReference(selection, d);
      if (!ref) return;
      if (has(C.OPT.align) && !anyGrouped(d, selection.objects)) {
        void exec(C.align(selection.objects, { reference: { kind: "object", id: ref }, x, y, mode: "each" }));
        return;
      }
      const refUnit = units.find((u) => u.members.includes(ref));
      if (!refUnit) return;
      sendMoves(
        alignMoves(
          units.filter((u) => u !== refUnit),
          refUnit.box,
          x,
          y,
        ),
        units,
      );
    },
    [selUnits, selection, has, exec, sendMoves, notify],
  );

  /** Move the whole selection so its centre lands on the stock's or the field's centre. */
  const centreOn = useCallback(
    (target: "stock" | "field") => {
      const d = docRef.current;
      if (!d || selection.objects.length === 0) return;
      if (target === "stock" && !d.stock) {
        notify(t("app.stockFirst"), "error");
        return;
      }
      if (has(C.OPT.align)) {
        void exec(C.align(selection.objects, { reference: { kind: target }, x: "center", y: "center", mode: "group" }));
        return;
      }
      const b = boxOf(selection.objects);
      if (!b) return;
      const c = boxCenter(b);
      const to: Vec2 = target === "stock" && d.stock ? d.stock.center : [0, 0];
      void exec(C.transform(selection.objects, { translate: [roundMm(to[0] - c[0]), roundMm(to[1] - c[1])] }));
    },
    [selection.objects, has, exec, boxOf, notify],
  );

  const distribute = useCallback(
    (axis: "x" | "y", by: "centers" | "gaps") => {
      const d = docRef.current;
      if (!d) return;
      const units = selUnits();
      if (units.length < 3) {
        notify(t("app.distributeThree"), "error");
        return;
      }
      if (has(C.OPT.distribute) && !anyGrouped(d, selection.objects)) {
        void exec(C.distribute(selection.objects, axis, by));
        return;
      }
      sendMoves(distributeMoves(units, axis, by), units);
    },
    [selUnits, has, exec, sendMoves, selection.objects, notify],
  );

  const group = useCallback(() => {
    const d = docRef.current;
    if (!d || selection.objects.length === 0) return;
    if (!has(C.OPT.groupCreate)) {
      notify(t("app.noGroup"), "error");
      return;
    }
    const units = unitsOf(d, selection.objects);
    if (units.length < 2) {
      notify(t("app.groupTwo"), "error");
      return;
    }
    void exec(C.groupCreate(units));
  }, [selection.objects, has, exec, notify]);

  const ungroup = useCallback(() => {
    const d = docRef.current;
    if (!d) return;
    const groups = [...new Set(selection.objects.map((id) => topGroup(d, id)).filter((g): g is string => g !== null))];
    if (groups.length === 0) {
      notify(t("app.noGroups"), "error");
      return;
    }
    if (!has(C.OPT.groupUngroup)) {
      notify(t("app.noUngroup"), "error");
      return;
    }
    const invs = groups.map((g) => C.groupUngroup(g));
    void exec(invs.length === 1 ? invs[0] : invs);
  }, [selection.objects, has, exec, notify]);

  const flip = useCallback(
    (axis: "x" | "y") => selection.objects.length > 0 && void exec(C.transform(selection.objects, { mirror: axis, about: "center" })),
    [selection.objects, exec],
  );

  const duplicate = useCallback(() => {
    if (selection.objects.length === 0) return;
    if (!has(C.OPT.duplicate)) {
      notify(t("app.noDuplicate"), "error");
      return;
    }
    run(C.duplicate(selection.objects))
      .then((r) => {
        const ids = createdIds(r);
        if (ids.length) selectIds(ids);
      })
      .catch(report);
  }, [selection.objects, has, run, selectIds, report, notify]);

  const deleteSelection = useCallback(() => {
    if (selection.objects.length) void exec(C.remove(selection.objects));
  }, [selection.objects, exec]);

  // Cut / copy / paste: the design layer keeps the internal copy (an Art Library fragment: images, groups, text and
  // parametric shapes survive a cut) and talks to the system clipboard (src/design/useDesignUi.tsx).
  const copy = useCallback((cut: boolean) => designRef.current.copy(cut), []);
  const paste = useCallback((inPlace: boolean) => designRef.current.paste(inPlace), []);

  // While a command is in flight, auto-repeats of the same held arrow key merge, so holding a key sends a few commands,
  // not dozens. Separate presses, or another key or modifier, are never merged: one undo step each (see `mergesNudge`).
  const nudgeQ = useRef<{ ids: string[]; stock: boolean; d: Vec2; target: string; key: string }[]>([]);
  const nudgeBusy = useRef(false);
  const flushNudge = useCallback(() => {
    if (nudgeBusy.current) return;
    const q = nudgeQ.current.shift();
    if (!q) return;
    nudgeBusy.current = true;
    const d: Vec2 = [roundMm(q.d[0]), roundMm(q.d[1])];
    const inv = q.stock ? C.stockMove({ translate: d }) : C.transform(q.ids, { translate: d });
    run(inv)
      .catch(report)
      .finally(() => {
        nudgeBusy.current = false;
        flushNudge();
      });
  }, [run, report]);
  const nudge = useCallback(
    (d: Vec2, key: string, repeat: boolean) => {
      const stock = selection.objects.length === 0 && selection.stock;
      if (!stock && selection.objects.length === 0) return;
      if (stock && !has(C.OPT.stockMove)) return;
      const target = stock ? "stock" : selection.objects.join(",");
      queueNudge(nudgeQ.current, { ids: selection.objects, stock, d, target, key }, repeat);
      flushNudge();
    },
    [selection, has, flushNudge],
  );

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

  const arrange = useCallback(
    (a: ArrangeAction) => {
      switch (a) {
        case "left":
        case "right":
          return alignTo(a, undefined);
        case "hcenter":
          return alignTo("center", undefined);
        case "top":
        case "bottom":
          return alignTo(undefined, a);
        case "vcenter":
          return alignTo(undefined, "center");
        case "centers":
          return alignTo("center", "center");
        case "distH":
          return distribute("x", "gaps");
        case "distV":
          return distribute("y", "gaps");
        case "distHc":
          return distribute("x", "centers");
        case "distVc":
          return distribute("y", "centers");
        case "stock":
          return centreOn("stock");
        case "field":
          return centreOn("field");
        case "group":
          return group();
        case "ungroup":
          return ungroup();
        case "flipH":
          return flip("x");
        case "flipV":
          return flip("y");
      }
    },
    [alignTo, distribute, centreOn, group, ungroup, flip],
  );
  arrangeRef.current = arrange;

  // ---- keyboard ----------------------------------------------------------------------------------------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && !e.altKey && k === "s") {
        // Never the browser's "save page"; Save prompts for a name the first time, Ctrl+Shift+S always.
        e.preventDefault();
        if (dialog || fileModalOpen || e.repeat) return;
        // Commit a field being edited (its value is sent on blur) before saving.
        if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
        if (e.shiftKey) file.setModal({ kind: "saveAs" });
        else saveFile();
        return;
      }
      if (typingIn(e.target)) return;
      if (dialog || fileModalOpen || filesUi.modalOpen || libsUi.modalOpen || designRef.current.modalOpen) return;
      const d = docRef.current;
      const design = workspace === "design";
      const m = { shift: e.shiftKey, ctrl: mod, alt: e.altKey };
      if (mod && !e.altKey) {
        switch (k) {
          case "z":
            e.preventDefault();
            if (e.shiftKey) void redo();
            else void undo();
            return;
          case "y":
            e.preventDefault();
            void redo();
            return;
          case "o":
            e.preventDefault();
            file.setModal({ kind: "open" });
            return;
          case "n":
            // Most browsers keep Ctrl+N for a new window; where they let it through, it is File > New.
            e.preventDefault();
            file.newDocument();
            return;
          case "i":
            e.preventDefault();
            if (e.shiftKey) {
              if (d && design) setSelection(invertSelection(selection, d));
            } else openImport();
            return;
          case "a":
            e.preventDefault();
            if (d && design) setSelection(selectAll(d));
            return;
        }
        if (design && !e.repeat) {
          if (e.shiftKey && (k === "h" || k === "v")) {
            e.preventDefault();
            flip(k === "h" ? "x" : "y");
            return;
          }
          switch (k) {
            case "g":
              e.preventDefault();
              group();
              return;
            case "u":
              e.preventDefault();
              ungroup();
              return;
            case "d":
              e.preventDefault();
              duplicate();
              return;
            case "c":
              if (selection.objects.length && window.getSelection()?.isCollapsed !== false) designRef.current.clipKey("copy", e);
              return;
            case "x":
              if (selection.objects.length) designRef.current.clipKey("cut", e);
              return;
            case "v":
              designRef.current.clipKey("paste", e);
              return;
          }
        }
      }
      if (e.key === "Escape") {
        setMenu(null);
        togglePositionLaser(false);
        if (tool !== "select") setTool("select");
        else setSelectionRaw(EMPTY_SELECTION);
        return;
      }
      if (!design) return;
      if (e.altKey && !mod && k === "v" && designRef.current.canPaste) {
        designRef.current.clipKey("pasteInPlace", e);
        return;
      }
      // Alt+arrows / Alt+PgUp/PgDn align (LightBurn).
      if (e.altKey && !mod && !e.shiftKey) {
        const al: Record<string, () => void> = {
          ArrowLeft: () => alignTo("left"),
          ArrowRight: () => alignTo("right"),
          ArrowUp: () => alignTo(undefined, "top"),
          ArrowDown: () => alignTo(undefined, "bottom"),
          PageUp: () => alignTo("center"),
          PageDown: () => alignTo(undefined, "center"),
        };
        if (al[e.key]) {
          e.preventDefault();
          al[e.key]();
          return;
        }
      }
      const nd = !e.altKey ? nudgeDelta(e.key, m, getPrefs().nudge) : null;
      if (nd) {
        if (selection.objects.length || selection.stock) {
          e.preventDefault();
          nudge(nd, nudgeKeyId(e.key, m), e.repeat);
        }
        return;
      }
      const rot = !e.altKey ? rotateKeyDeg(e.key, m) : null;
      if (rot !== null && selection.objects.length) {
        e.preventDefault();
        void exec(C.transform(selection.objects, { rotate_deg: rot, about: "center" }));
        return;
      }
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteSelection();
        return;
      }
      if (e.key === "Tab" && canvasFocused() && d) {
        e.preventDefault();
        setSelection(cycleSelection(selection, d, e.shiftKey ? -1 : 1));
        return;
      }
      if (mod || e.altKey || !canvasFocused()) return;
      // Canvas keys: with two or more units selected L/R/T/B/C/E align (LightBurn); P centres on the field.
      const multi = d ? unitsOf(d, selection.objects).length >= 2 : false;
      if (multi && !e.shiftKey) {
        const al: Record<string, () => void> = {
          l: () => alignTo("left"),
          r: () => alignTo("right"),
          t: () => alignTo(undefined, "top"),
          b: () => alignTo(undefined, "bottom"),
          c: () => alignTo("center"),
          e: () => alignTo(undefined, "center"),
        };
        if (al[k]) {
          e.preventDefault();
          al[k]();
          return;
        }
      }
      if (k === "p" && selection.objects.length) {
        centreOn("field");
        return;
      }
      if (k === "r") setTool("rect");
      else if (k === "e") setTool("ellipse");
      else if (k === "c") setTool("circle");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    dialog,
    fileModalOpen,
    filesUi.modalOpen,
    libsUi.modalOpen,
    saveFile,
    file,
    tool,
    workspace,
    undo,
    redo,
    deleteSelection,
    selection,
    setSelection,
    openImport,
    group,
    ungroup,
    duplicate,
    copy,
    paste,
    flip,
    alignTo,
    centreOn,
    nudge,
    exec,
  ]);

  // LightBurn: Alt+Shift+W toggles the filled view; Alt+Shift+M opens the Devices window.
  const canDevices = !devices.unsupported;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || !e.shiftKey || typingIn(e.target)) return;
      const k = e.key.toLowerCase();
      if (k === "w") {
        e.preventDefault();
        setPrefs((p) => ({ ...p, viewStyle: p.viewStyle === "filled" ? "wireframe" : "filled" }));
      } else if (k === "m" && canDevices) {
        e.preventDefault();
        setDevicesOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canDevices]);

  const onPick = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files ?? []);
    e.target.value = "";
    if (files.length) void handleFiles(files);
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
  // After a device switch: the profile, field size and controller changed (the `device_changed` event does the same).
  const onDeviceSwitched = useCallback(() => {
    void engine.refreshMachine();
    void engine.refreshDoc();
  }, [engine]);
  const selectedLayer = selection.layer ? doc?.layers.find((l) => l.id === selection.layer) : undefined;
  const hasSel = selection.objects.length > 0;
  const units = doc ? unitsOf(doc, selection.objects).length : 0;
  const grouped = doc ? anyGrouped(doc, selection.objects) : false;

  const menuItems: MenuItem[] = [
    { label: t("app.cut"), hint: "Ctrl+X", disabled: !hasSel, action: () => copy(true) },
    { label: t("app.copy"), hint: "Ctrl+C", disabled: !hasSel, action: () => copy(false) },
    { label: t("app.paste"), hint: "Ctrl+V", disabled: !designUi.canPaste, action: () => paste(false) },
    { label: t("app.duplicate"), hint: "Ctrl+D", disabled: !hasSel || !has(C.OPT.duplicate), action: duplicate },
    { label: t("app.delete"), hint: "Del", disabled: !hasSel, action: deleteSelection },
    { sep: true },
    { label: t("app.group"), hint: "Ctrl+G", disabled: units < 2 || !has(C.OPT.groupCreate), action: group },
    { label: t("app.ungroup"), hint: "Ctrl+U", disabled: !grouped || !has(C.OPT.groupUngroup), action: ungroup },
    { sep: true },
    ...edit.menuItems,
    ...filesUi.contextItems,
    ...libsUi.menuItems,
    ...designUi.menuItems,
    ...(text.menuItems.length ? [{ sep: true } as const, ...text.menuItems] : []),
    { sep: true },
    { label: t("app.alignLeft"), hint: "L", disabled: units < 2, action: () => alignTo("left") },
    { label: t("app.alignHCenter"), hint: "C", disabled: units < 2, action: () => alignTo("center") },
    { label: t("app.alignRight"), hint: "R", disabled: units < 2, action: () => alignTo("right") },
    { label: t("app.alignTop"), hint: "T", disabled: units < 2, action: () => alignTo(undefined, "top") },
    { label: t("app.alignVCenter"), hint: "E", disabled: units < 2, action: () => alignTo(undefined, "center") },
    { label: t("app.alignBottom"), hint: "B", disabled: units < 2, action: () => alignTo(undefined, "bottom") },
    { label: t("app.centreStock"), disabled: !hasSel || !doc?.stock, action: () => centreOn("stock") },
    { label: t("app.centreField"), hint: "P", disabled: !hasSel, action: () => centreOn("field") },
    { sep: true },
    ...docking.menuItems,
    { sep: true },
    { label: t("app.flipH"), hint: "Ctrl+Shift+H", disabled: !hasSel, action: () => flip("x") },
    { label: t("app.flipV"), hint: "Ctrl+Shift+V", disabled: !hasSel, action: () => flip("y") },
    { sep: true },
    { label: t("app.selectAll"), hint: "Ctrl+A", disabled: !doc, action: () => doc && setSelection(selectAll(doc)) },
    { label: t("app.invertSelection"), hint: "Ctrl+Shift+I", disabled: !doc, action: () => doc && setSelection(invertSelection(selection, doc)) },
  ];

  // The framing hotkeys: the Laser window's framing options (components/FramingOptions.tsx), on the selection when
  // there is one, else the whole job; `style` overrides the frame style.
  const frameWith = (style?: "bounds" | "outline") =>
    void machineAction(frameWithOptions(selection.objects, { selectionOnly: selection.objects.length > 0, ...(style ? { style } : {}) }), t("app.framing"));
  // Request run (hotkey, toolbar) with the Laser window's run options: it still needs PIN approval, or without a PIN
  // (`--approval click`) it is LightBurn's Start and burns at once (the Job Checklist is ticked in the Laser window).
  const requestRun = () =>
    void runJob(noPinMode(machine), selection.objects, null, null, doc?.revision ?? null).then(notify).catch(report).finally(() => void engine.refreshMachine());
  const laserDisabled = designOnlyReason(device);
  // Workspace hotkeys (workspace/hotkeys.ts); App's own keys stay in the handler above.
  ws.keymap.current = {
    blocked: () => !!dialog || fileModalOpen || devicesOpen || filesUi.modalOpen || libsUi.modalOpen || designUi.modalOpen,
    modalOpen: () => ws.dialog !== null,
    handlers: {
      "app.preferences": () => ws.open("prefs"),
      "help.hotkeys": () => ws.open("hotkeys"),
      // Ctrl+R / Ctrl+E and rebound tool keys pick the tool even when R/E/C would align a multi-selection.
      "tool.rect": () => (workspace === "design" ? setTool("rect") : false),
      "tool.ellipse": () => (workspace === "design" ? setTool("ellipse") : false),
      "tool.circle": () => (workspace === "design" ? setTool("circle") : false),
      "view.preview": () => {
        if (workspace === "laser") return switchWorkspace("design");
        switchWorkspace("laser");
        setShowPreview(true);
      },
      "view.zoomIn": () => sendViewCommand("zoomIn"),
      "view.zoomOut": () => sendViewCommand("zoomOut"),
      "view.zoomPage": () => sendViewCommand("fit"),
      "view.zoomSelection": () => sendViewCommand("selection"),
      "laser.frameSelection": () => (laserDisabled ? false : frameWith()),
      "laser.frameSquare": () => (laserDisabled ? false : frameWith("bounds")),
      "laser.frameOutline": () => (laserDisabled ? false : frameWith("outline")),
      "laser.frameStop": () => void machineAction(api.frameStop(), t("app.frameStopped")),
      "laser.requestRun": () => (laserDisabled ? false : requestRun()),
      "laser.pauseResume": () => void togglePause().then(notify).catch(report),
      "laser.stop": stop,
      ...text.handlers,
      ...filesUi.handlers,
      ...libsUi.handlers,
      ...designUi.handlers,
      ...edit.handlers,
      // LightBurn 2.0 defaults (workspace/hotkeys.ts): Trim Ctrl+K, Measure Alt+M, Align centres Alt+Home, Move together
      // Alt+Shift+H / V, Position laser Alt+L, Rotary setup Ctrl+Shift+R; Docking and Toggle Side Panels unbound.
      "tool.trim": () => (workspace === "design" && has("path.trim") ? setTool("trim") : false),
      "tool.measure": () => (workspace === "design" ? setTool("measure") : false),
      "arrange.alignCenters": () => (workspace === "design" && units >= 2 ? alignTo("center", "center") : false),
      "arrange.distHSpaced": () => (workspace === "design" && units >= 3 && !beginnerMode ? arrange("distH") : false),
      "arrange.distHCentered": () => (workspace === "design" && units >= 3 && !beginnerMode ? arrange("distHc") : false),
      "arrange.distVSpaced": () => (workspace === "design" && units >= 3 && !beginnerMode ? arrange("distV") : false),
      "arrange.distVCentered": () => (workspace === "design" && units >= 3 && !beginnerMode ? arrange("distVc") : false),
      "arrange.moveHTogether": () => (workspace === "design" && units >= 2 ? setMoveTogether("x") : false),
      "arrange.moveVTogether": () => (workspace === "design" && units >= 2 ? setMoveTogether("y") : false),
      "arrange.dockLeft": () => (workspace === "design" && hasSel ? docking.dock("left") : false),
      "arrange.dockRight": () => (workspace === "design" && hasSel ? docking.dock("right") : false),
      "arrange.dockUp": () => (workspace === "design" && hasSel ? docking.dock("up") : false),
      "arrange.dockDown": () => (workspace === "design" && hasSel ? docking.dock("down") : false),
      "laser.position": () => (laserDisabled ? false : togglePositionLaser()),
      "laser.rotary": () => {
        if (!has("job.set_rotary")) return false;
        if (workspace !== "laser") switchWorkspace("laser");
        requestRotarySetup();
      },
      "window.sidePanels": () => {
        const open = !(leftOpen || rightOpen);
        setLeftOpen(open);
        setRightOpen(open);
      },
      "window.newTab": () => (tabs.api.supported ? tabs.api.newTab() : false),
    },
  };

  const onSelectObject = (id: string, op: SelectOp | boolean) => {
    if (!doc) return;
    const o: SelectOp = typeof op === "boolean" ? (op ? "toggle" : "replace") : op;
    setSelection(applySelect(selection, doc, [id], o));
  };

  return (
    <div className={`app ws-${workspace}${narrow ? " narrow" : ""}`}>
      <TopBar
        fileMenu={
          <FileMenu
            file={file}
            onImport={openImport}
            onDevices={canDevices ? () => setDevicesOpen(true) : undefined}
            recent={(close) => (
              <RecentProjects
                current={file.projectName}
                disabled={file.busy}
                onOpen={(name) => {
                  close();
                  file.openProject(name);
                }}
              />
            )}
            extra={(close) => (
              <>
                {tabs.api.supported && (
                  <button
                    role="menuitem"
                    className="menu-item"
                    onClick={() => {
                      close();
                      tabs.api.newTab();
                    }}
                  >
                    <span>{t("tabs.newTab")}</span>
                  </button>
                )}
                <WorkspaceFileItems close={close} open={ws.open} />
                {libsUi.fileItems(close)}
              </>
            )}
            files={filesUi.menu}
            onSaveGcode={
              isGrblMachine(machine)
                ? () =>
                    void jobApi
                      .exportGcode()
                      .then(({ blob, filename }) => {
                        const name = gcodeFileName(filename, file.projectName ?? file.openedName ?? doc?.name ?? null);
                        saveBlob(blob, name);
                        notify(t("fm.gcodeSaved", { name }));
                      })
                      .catch(report)
                : undefined
            }
          />
        }
        extra={
          <>
            <WindowMenu />
            <SimPinBadge machine={machine} />
            <ShutdownBadge machine={machine} />
            <TopActions machine={machine} onHotkeys={() => ws.open("hotkeys")} onPreferences={() => ws.open("prefs")} helpExtra={libsUi.helpItems} />
          </>
        }
        workspace={workspace}
        onWorkspace={switchWorkspace}
        onUndo={() => void undo()}
        onRedo={() => void redo()}
        onStop={stop}
        link={engine.link}
        httpError={engine.httpError}
        machine={machine}
        missingCommands={missingCommands}
        docName={file.openedName ?? doc?.name ?? null}
        projectName={file.projectName}
        dirty={file.dirty}
        pendingRuns={runs.length}
      />
      <div className="doc-tabs-row">{tabs.bar}</div>
      <Toolbar
        workspace={workspace}
        tool={tool}
        hasSelection={hasSel}
        units={units}
        grouped={grouped}
        hasStock={!!doc?.stock}
        showPreview={showPreview}
        leftOpen={leftOpen}
        rightOpen={rightOpen}
        snap={prefs.snap}
        viewStyle={prefs.viewStyle}
        canGroup={has(C.OPT.groupCreate)}
        onTool={setTool}
        onStock={() => setDialog("stock")}
        onImport={openImport}
        onPlace={() => setDialog("place")}
        onArray={designUi.openArray}
        onDelete={deleteSelection}
        onArrange={arrange}
        onSnap={(snap) => setPrefs((p) => ({ ...p, snap }))}
        onViewStyle={(viewStyle) => setPrefs((p) => ({ ...p, viewStyle }))}
        onCommands={() => setDialog("commands")}
        onTogglePreview={() => setShowPreview((v) => !v)}
        onFrame={() => void machineAction(frameWithOptions(selection.objects), t("app.framing"))}
        onFrameStop={() => void machineAction(api.frameStop(), t("app.frameStopped"))}
        onRequestRun={requestRun}
        startMode={noPinMode(machine)}
        designOnly={designOnlyReason(device)}
        onToggleLeft={() => setLeftOpen((v) => !v)}
        onToggleRight={() => setRightOpen((v) => !v)}
        extra={
          <>
            {text.toolbar}
            {edit.toolbar}
            {designUi.toolbar}
            {libsUi.toolbar}
          </>
        }
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
        <aside className={`panel panel-left${leftOpen && leftHasWindows ? " open" : ""}`} aria-label={t("win.browser")}>
          <div className="panel-body">
            <DockAreaHost area="left" />
            <Dock id="browser" title={t("win.browser")}>
            <BrowserPanel
              doc={doc}
              selection={selection}
              has={has}
              onSelectObject={onSelectObject}
              onSelectLayer={(id, all) => {
                if (all && doc) setSelection(selectLayerObjects(doc, id));
                else setSelection({ objects: [], layer: id, stock: false });
              }}
              onSelectStock={() => setSelection({ objects: [], layer: null, stock: true })}
              onObjectMenu={(x, y, id) => {
                if (doc && !selection.objects.includes(id)) setSelection(applySelect(selection, doc, [id], "replace"));
                setMenu({ x, y });
              }}
              run={run}
              report={report}
            />
            </Dock>
          </div>
        </aside>

        <section className="center">
          {workspace === "design" && <NumericBar doc={doc} selection={selection} run={run} report={report} />}
          <div className="center-stage">
          <Viewport
            doc={doc}
            workspaceMm={workspaceMm}
            selection={selection}
            tool={tool}
            activeLayer={activeLayer}
            preview={engine.preview}
            showPreview={previewOn}
            previewOptions={previewOptions}
            viewStyle={prefs.viewStyle}
            snap={prefs.snap}
            outside={outside}
            has={has}
            onSelection={setSelection}
            onToolDone={() => setTool("select")}
            onContextMenu={(x, y) => setMenu({ x, y })}
            onCursor={(p) => (cursor.current = p)}
            run={run}
            notify={notify}
            report={report}
            canvasTool={withPositionLaser(
              withLayerFlash(designUi.wrap(libsUi.wrap(text.wrap(edit.canvas))), doc, flash),
              positioning,
              (pt) =>
                void machineAction(jobApi.pointer(roundMm(pt[0]), roundMm(pt[1])), t("poslaser.done", { x: roundMm(pt[0]), y: roundMm(pt[1]) })),
              (end) => (
                <div className="vp-overlay lx-sp-bar" role="status">
                  <span>{t("poslaser.bar")}</span>
                  <button className="btn small" onClick={end}>
                    {t("common.cancel")}
                  </button>
                </div>
              ),
            )}
          />
          {previewOn && engine.preview && (
            <PreviewTimeline
              preview={engine.preview}
              options={previewOptions}
              onOptions={setPreviewOptions}
              designOnly={laserDisabled}
              docName={file.projectName ?? file.openedName ?? doc?.name ?? null}
              notify={notify}
              report={report}
              onRequested={() => void engine.refreshMachine()}
              noPin={noPinMode(machine)}
              docRevision={doc?.revision ?? null}
            />
          )}
          </div>
          <DockAreaHost area="bottom" />
          {workspace === "design" && <PaletteBar doc={doc} selected={selection.objects} run={run} report={report} notify={notify} has={has} />}
        </section>

        <aside className={`panel panel-right${rightOpen && rightHasWindows ? " open" : ""}`} aria-label={workspace === "laser" ? t("win.machine") : t("win.properties")}>
          <div className="panel-body">
            <DockAreaHost area="right" />
            {workspace === "design" ? (
              <>
              {filesUi.pagesDock}
              <CutsLayers
                doc={doc}
                selection={selection}
                machine={machine}
                has={has}
                run={run}
                report={report}
                notify={notify}
                onSelectLayer={(id, all) => {
                  if (all && doc) setSelection(selectLayerObjects(doc, id));
                  else setSelection({ objects: [], layer: id, stock: false });
                }}
              />
              {libsUi.dock}
              {designUi.dock}
              {docking.window}
              <Dock id="properties" title={t("win.properties")}>
              <PropertiesPanel
                doc={doc}
                selection={selection}
                machine={machine}
                workspaceMm={workspaceMm}
                has={has}
                run={run}
                report={report}
                onEditStock={() => setDialog("stock")}
                onArrange={arrange}
              />
              {designUi.properties}
              {text.properties}
              </Dock>
              </>
            ) : (
              <div className="props">
                <LaserDock
                  machine={machine}
                  httpError={engine.httpError}
                  device={
                    canDevices ? (
                      <DeviceBar
                        devices={devices}
                        machine={machine}
                        onOpenWindow={() => setDevicesOpen(true)}
                        onSwitched={onDeviceSwitched}
                        notify={notify}
                        report={report}
                      />
                    ) : undefined
                  }
                  designOnly={laserDisabled}
                  link={engine.link}
                  doc={doc}
                  selection={selection.objects}
                  pendingRuns={runs.length}
                  has={has}
                  run={run}
                  notify={notify}
                  report={report}
                  onChanged={() => void engine.refreshMachine()}
                  onStop={stop}
                  extras={<LaserExtras designOnly={laserDisabled} tools={libsUi.laserTools} notify={notify} report={report} />}
                />
                <CutsLayers
                  doc={doc}
                  selection={selection}
                  machine={machine}
                  has={has}
                  run={run}
                  report={report}
                  notify={notify}
                  onSelectLayer={(id, all) => {
                    if (all && doc) setSelection(selectLayerObjects(doc, id));
                    else setSelection({ objects: [], layer: id, stock: false });
                  }}
                />
                {libsUi.dock}
                {/* A GRBL gantry's Move and Console windows (LightBurn's); galvos have neither. */}
                {isGrblMachine(machine) && (
                  <>
                    <Dock id="move" title={t("win.move")}>
                      <GrblMove notify={notify} report={report} />
                    </Dock>
                    <Dock id="console" title={t("win.console")}>
                      <GrblConsole notify={notify} report={report} />
                    </Dock>
                  </>
                )}
                <Dock id="machine" title={t("win.machine")}>
                <MachineCard machine={machine} httpError={engine.httpError}>
                  {/* Engines without the device registry: LightBurn's devices and the profile diffs. */}
                  {!canDevices && (
                    <LightBurnDevices
                      lightburn={lightburn}
                      machine={machine}
                      onChanged={() => void engine.refreshMachine()}
                      notify={notify}
                      report={report}
                    />
                  )}
                </MachineCard>
                <RunsCard
                  machine={machine}
                  docRevision={doc?.revision ?? null}
                  onChanged={() => void engine.refreshMachine()}
                  report={report}
                />
                <PreviewStatsCard
                  preview={engine.preview}
                  error={engine.previewError ? friendlyError(engine.previewError, doc) : null}
                  enabled={previewOn}
                  doc={doc}
                  options={previewOptions}
                  onOptions={setPreviewOptions}
                />
                {selectedLayer && <LayerSection layer={selectedLayer} machine={machine} run={run} report={report} has={has} />}
                </Dock>
              </div>
            )}
          </div>
        </aside>
      </div>
      <Timeline
        timeline={engine.timeline}
        flash={engine.remoteEdit}
        onGoto={(marker) => void engine.historyGoto(marker).catch(report)}
        onDelete={(index) => void engine.historyDelete(index).catch(report)}
      />
      <StatusBar doc={doc} selection={selection} snap={prefs.snap} machineText={statusMachineText(machineState(machine), t)} />

      <input ref={fileRef} type="file" accept={importAccept(has("art.import_pdf") ? ENGINE_ACCEPT : null)} multiple hidden onChange={onPick} />

      {dialog === "stock" && <StockDialog doc={doc} run={run} onClose={closeDialog} />}
      {dialog === "place" && <PlaceDialog ids={selection.objects} run={run} onClose={closeDialog} />}
      {dialog === "commands" && <CommandPalette commands={engine.commands} run={run} onClose={closeDialog} />}
      {moveTogether && <MoveTogetherDialog ids={selection.objects} axis={moveTogether} run={run} onClose={() => setMoveTogether(null)} />}
      {edit.dialogs}
      {filesUi.dialogs}
      {libsUi.dialogs}
      {designUi.dialogs}
      {text.dialogs}
      {ws.dialogs}
      <FileDialogs file={file} docName={doc?.name ?? null} />
      {devicesOpen && (
        <DevicesWindow
          devices={devices}
          machine={machine}
          onClose={() => setDevicesOpen(false)}
          onSwitched={onDeviceSwitched}
          notify={notify}
          report={report}
          doc={doc}
          run={run}
        />
      )}
      <FloatHost />
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />}
      {dropLabel && <DropOverlay label={dropLabel} />}

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

/** The machine part of the status bar. */
function statusMachineText(st: ReturnType<typeof machineState>, tr: ReturnType<typeof useT>): string {
  const t = {
    offline: tr("app.stOffline"),
    framing: tr("app.stFraming"),
    running: tr("app.stRunning"),
    busy: tr("app.stBusy"),
    ready: tr("app.stReady"),
    noLaser: tr("app.stNoLaser"),
  }[st.text];
  return st.sim && st.text === "ready" ? tr("app.stSim", { state: t }) : t;
}
