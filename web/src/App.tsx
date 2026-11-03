// Application shell: top bar, toolbar, Browser | Viewport | Properties, Timeline, laid out per layout mode
// (workspace/layoutMode.ts, docs/ux/ui-plan-2026-11.md 1.1): wide, compact (MacBook), tablet and phone. The side
// panels are pinned beside the canvas or rails that open as overlays (workspace/layout.ts `dockPolicy`); the phone gets
// its own shell (workspace/PhoneShell.tsx). All document state comes from the engine; the only client state is the
// view: workspace, tool, selection, clipboard, open panels and dialogs, display options.
//
// Files dropped anywhere on the window (or picked with Import / Ctrl+I) are routed by api/drop.ts: SVG, DXF and bitmaps
// are imported into the document, a LightBurn .lbrn2 opens the Open/Import dialog, a .openetch opens the project.
//
// The canvas shows the active device's workspace (components/Devices.tsx): a galvo's square field or a gantry
// device's rectangle. File > Devices… (Alt+Shift+M, as in LightBurn) opens the Devices window, and the Laser workspace
// has the device dropdown.

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
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
import { layerHasImages } from "./api/cutOptions.ts";
import { estimateOf } from "./api/packedPreview.ts";
import { activeDevice, asLightBurnDevices, canvasWorkspace, designOnlyReason, isGalvo, machineDevice } from "./api/devices.ts";
import { engineLightBurnDevice } from "./api/lightburn.ts";
import { MAX_UPLOAD_BYTES, commandResult, defaultImportMode, fileError, warningList } from "./api/project.ts";
import { activeRuns, noPinMode, pendingRuns, stopToast } from "./api/runs.ts";
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
import { useTemplateUi } from "./templates/useTemplateUi.tsx";
import { ShutdownBadge, SimPinBadge } from "./libs/JobSafety.tsx";
import { LaserExtras } from "./libs/LaserExtras.tsx";
import { useLibsUi } from "./libs/useLibsUi.tsx";
import { useProductionUi } from "./production/useProductionUi.tsx";
import { useMaterialsUi } from "./materials/useMaterialsUi.tsx";
import { useToolsUi } from "./tools/useToolsUi.tsx";
import { useJigsUi } from "./jigs/useJigsUi.tsx";
import { useFillRegionTool } from "./fill/useFillRegionTool.tsx";
import { useDesignUi } from "./design/useDesignUi.tsx";
import { MachineCard, PreviewStatsCard, RunsCard } from "./components/LaserPanel.tsx";
import { frameWithOptions } from "./components/FramingOptions.tsx";
import { togglePause } from "./components/JobControls.tsx";
import { PreviewTimeline } from "./components/PreviewTimeline.tsx";
import { ReliefHost, openRelief } from "./relief/ReliefView.tsx";
import { LightBurnDevices, useLightBurn } from "./components/LightBurn.tsx";
import { LayerSection, PropertiesPanel } from "./components/PropertiesPanel.tsx";
import { Timeline } from "./components/Timeline.tsx";
import { Toolbar, type ArrangeAction } from "./components/Toolbar.tsx";
import { runHotkeyAction } from "./components/commands.ts";
import { StartButton } from "./laser/StartButton.tsx";
import { TopBar, type Workspace } from "./components/TopBar.tsx";
import { Icon } from "./components/ui.tsx";
import { Viewport, type PreviewOptions, type SnapSettings, type Tool, type ViewStyle } from "./components/Viewport.tsx";
import { CutsLayers } from "./workspace/CutsLayers.tsx";
import { AboutDialog, HelpItems, TopActions } from "./workspace/HelpMenu.tsx";
import { PHONE_SEARCH_EVENT, PhoneShell } from "./workspace/PhoneShell.tsx";
import { LaserDock, machineState } from "./workspace/LaserDock.tsx";
import { NumericBar } from "./workspace/NumericBar.tsx";
import { PaletteBar } from "./workspace/PaletteBar.tsx";
import { getPrefs, updatePrefs } from "./workspace/prefs.ts";
import { hintFor, stopChordActs } from "./workspace/hotkeys.ts";
import { VIEW_PREFS_EVENT } from "./files/LightBurnPrefsImport.tsx";
import { RecentProjects } from "./workspace/RecentProjects.tsx";
import { StatusBar } from "./workspace/StatusBar.tsx";
import { sendViewCommand } from "./workspace/viewStatus.ts";
import { WorkspaceFileItems, useWorkspaceUi } from "./workspace/useWorkspace.tsx";
import { Dock } from "./workspace/Dock.tsx";
import { DockAreaHost, FloatHost, SidePanel, Splitter, WindowMenu, WindowMenuEntries, useAreaHasWindows, useWindowSize } from "./workspace/DockHost.tsx";
import { dockPolicy, setAreaMode, updateLayout, type AreaMode, type SideArea } from "./workspace/layout.ts";
import { useLayout } from "./workspace/windows.ts";
import { useFullscreen } from "./workspace/fullscreen.ts";
import { appModeDialogOpen, openAppModeDialog } from "./app/AppModeDialog.tsx";
import { useLayoutMode } from "./workspace/layoutMode.ts";
import { desktopFeatures, forLayout } from "./workspace/desktopOnly.ts";
import { ContextStripHost, useContextStripBusy } from "./workspace/contextStrip.tsx";
import { bottomRow, windowTitle } from "./workspace/shell.ts";
import { syncThemeColor } from "./app/appMode.ts";
import { openCommandSearch } from "./components/commandBus.ts";
import { OPEN_LASER_EVENT, useLaserBridge } from "./laser/laserContext.ts";
import { startFromHotkey } from "./laser/startFlow.ts";
import { pickerAccept } from "./touch/filePick.ts";
import { ConnectionBanner } from "./laser/ConnectionBanner.tsx";
import { EmptyCanvasCard } from "./laser/EmptyCanvasCard.tsx";
import { TouchHud } from "./touch/TouchHud.tsx";
import { CanvasHud, HudItem, useHudFixed } from "./hud/CanvasHud.tsx";
import { useToastHost } from "./components/useToastHost.ts";
import { useDocTabs } from "./workspace/DocTabs.tsx";
import { useDocking } from "./workspace/DockingTool.tsx";
import { useFlash, withLayerFlash } from "./workspace/flash.tsx";
import { togglePositionLaser, usePositionLaser, withPositionLaser } from "./workspace/positionLaser.tsx";
import { requestRotarySetup } from "./workspace/LaserDock.tsx";
import { MoveTogetherDialog } from "./design/DesignDialogs.tsx";
import { jobApi, saveBlob } from "./api/jobApi.ts";
import { gcodeFileName, isGrblMachine } from "./api/grbl.ts";
import { noteHeadPosition } from "./api/grblPositions.ts";
import { canvasKeysFocused } from "./workspace/keyFocus.ts";
import { GrblConsole } from "./components/GrblConsole.tsx";
import { GrblMove } from "./components/GrblMove.tsx";
import { RuidaFiles } from "./components/RuidaFiles.tsx";
import { isRuidaMachine, rdFileName } from "./api/ruida.ts";
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
import { errorText } from "./api/errorText.ts";

/** Field size until /api/machine reports `field_mm`. */
const DEFAULT_FIELD_MM = 153.75;
const VIEW_KEY = "openetch.view";

type Dialog = "stock" | "place" | "commands" | null;
interface Toast {
  id: number;
  msg: string;
  kind: "info" | "error";
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
  return canvasKeysFocused();
}

export function App() {
  const [workspace, setWorkspace] = useState<Workspace>("design");
  const [tool, setTool] = useState<Tool>("select");
  const [selection, setSelectionRaw] = useState<Selection>(EMPTY_SELECTION);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [showPreview, setShowPreview] = useState(true);
  const [toasts, setToasts] = useState<Toast[]>([]);
  // Toasts take the canvas HUD's top centre after the required controls are placed (the optional ones make room for
  // them), so they never cover one.
  const toastsRef = useHudFixed<HTMLDivElement>({ id: "toasts", slot: "tc", rank: 90, minW: 200, late: true, stack: true });
  const toastDialog = useToastHost();
  const [prefs, setPrefs] = useState<ViewPrefs>(loadPrefs);
  const [previewOptions, setPreviewOptions] = useState<PreviewOptions>({ hidden: new Set(), hiddenPasses: new Set(), upTo: null, shade: true, jumps: true });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [devicesOpen, setDevicesOpen] = useState(false);
  // Layout mode (workspace/layoutMode.ts): wide, compact (MacBook), tablet, phone; the pointer and the density.
  const { layout } = useLayoutMode();
  // Side panels shown (the toolbar's Browser / Properties toggles, Window > Toggle side panels); pinned or rail is the
  // layout's business (dockPolicy below). `overlay`: a rail opened over the canvas. `focus`: Focus canvas.
  const [leftOpen, setLeftOpen] = useState(true);
  const [rightOpen, setRightOpen] = useState(true);
  const [overlay, setOverlay] = useState<SideArea | null>(null);
  const [focus, setFocus] = useState(false);
  const [about, setAbout] = useState(false);
  const stripBusy = useContextStripBusy();
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
  const report = useCallback((e: unknown) => notify(errorText(e, docRef.current), "error"), [notify]);

  // A rail's overlay never outlives a change of layout.
  useEffect(() => setOverlay(null), [layout]);

  useEffect(() => setSelectionRaw((s) => pruneSelection(s, doc)), [doc]);

  // Selecting never opens a drawer (it covered the canvas on tablets): the Properties pill or the phone's peek sheet
  // shows the selection's settings on request.
  const setSelection = useCallback((s: Selection) => setSelectionRaw(s), []);
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
  // Show Last Position belongs to a GRBL gantry: on any other device the canvas forgets the head (the Move window that
  // would have cleared it on device_changed leaves with the GRBL device).
  const grblDevice = isGrblMachine(machine);
  useEffect(() => {
    if (!grblDevice) noteHeadPosition(null);
  }, [grblDevice]);
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
  // Templates with fill-in fields, example projects and the contact sheet (src/templates/).
  const tpl = useTemplateUi({ engine, file, selection: selection.objects, design: workspace === "design", notify, report });
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
  // The production cockpit (#/cockpit), serial ledger, layer pause HUD, maintenance chip (src/production/).
  const prodUi = useProductionUi({ notify, report, revision: doc?.revision ?? null });
  // MOPA materials: finishes, bracket test, colour separation, Result colours view (src/materials/).
  const materialsUi = useMaterialsUi({ doc, selection: selection.objects, design: workspace === "design", notify, report, selectIds });
  // Tabs, dogbone, Resize Slots, Multi-File Trace and the Web Tools (User Tools) menu (src/tools/).
  const toolsUi = useToolsUi({
    doc,
    selection: selection.objects,
    activeLayer,
    design: workspace === "design",
    run: engine.run,
    notify,
    report,
    selectIds,
    onTabTool: () => setTool("select"),
  });
  // Jigs and placement: Fit to stock, jigs with pockets, Re-mark region, pre-flight next to Start (src/jigs/).
  const jigsUi = useJigsUi({ doc, selection: selection.objects, design: workspace === "design", run: engine.run, notify, report, selectIds, onCanvasTool: () => setTool("select") });
  // Shape Properties, warp / deform, tangent circle, rubber band, toolpath to path, selection queries, page positions,
  // grid array, curve tolerance, the Art Library and the clipboard (src/design/).
  const designUi = useDesignUi({
    doc,
    selection,
    selectIds,
    activeLayer,
    workspaceMm,
    design: workspace === "design",
    blocked: !!dialog || fileModalOpen || devicesOpen || filesUi.modalOpen || libsUi.modalOpen || toolsUi.modalOpen,
    has,
    run: engine.run,
    notify,
    report,
    cursor,
    refreshDoc: () => void engine.refreshDoc(),
    extraTools: toolsUi.toolsItems,
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
  const bottomHasWindows = useAreaHasWindows("bottom");
  const dockLayout = useLayout();
  const view = useWindowSize();
  const fullscreen = useFullscreen();
  // What each side is now: pinned, a rail, or hidden (closed with its toggle, empty, Focus canvas, or a phone).
  const policy = dockPolicy(view.w, dockLayout.sizes, { layout, modes: dockLayout.areaMode, reserved: layout === "tablet" ? 52 : 0 });
  const sideMode = (side: SideArea): AreaMode => {
    const has = side === "left" ? leftHasWindows : rightHasWindows;
    const open = side === "left" ? leftOpen : rightOpen;
    return layout === "phone" || focus || !has || !open ? "hidden" : policy[side];
  };
  const leftMode = sideMode("left");
  const rightMode = sideMode("right");
  // The toolbar's Browser / Properties buttons: a rail opens its overlay; a pinned side hides; a hidden one comes back.
  const toggleSide = (side: SideArea) => {
    const mode = side === "left" ? leftMode : rightMode;
    const setOpen = side === "left" ? setLeftOpen : setRightOpen;
    if (mode === "rail") return setOverlay((o) => (o === side ? null : side));
    if (mode === "pinned") {
      if (overlay === side) setOverlay(null);
      return setOpen(false);
    }
    if (dockLayout.areaMode[side] === "hidden") updateLayout((l) => setAreaMode(l, side, null));
    setFocus(false);
    setOpen(true);
    if (policy[side] === "rail") setOverlay(side);
  };
  const setLeftOverlay = useCallback((o: boolean) => setOverlay((cur) => (o ? "left" : cur === "left" ? null : cur)), []);
  const setRightOverlay = useCallback((o: boolean) => setOverlay((cur) => (o ? "right" : cur === "right" ? null : cur)), []);

  // LightBurn's Arrange > Distribute (H-Spaced / H-Centered / V-Spaced / V-Centered), hidden in Beginner Mode like
  // LightBurn's. The arrange handler is defined further down; the menu reaches it through a ref.
  const { beginnerMode, hotkeys: prefHotkeys, timelinePinned, theme: themePref } = usePrefs();
  // Timeline and status bar: two rows (wide, pinned compact) or one merged row (compact, tablet).
  const row = bottomRow(layout, timelinePinned);
  useEffect(() => {
    document.documentElement.dataset.bottom = row;
  }, [row]);
  const arrangeRef = useRef<(a: ArrangeAction) => void>(() => {});
  const nUnits = doc ? unitsOf(doc, selection.objects).length : 0;
  const distributeItems: MenuItem[] = beginnerMode
    ? []
    : [
        { sep: true },
        { label: t("app.distHSpaced"), actionId: "arrange.distHSpaced", hint: hintFor("arrange.distHSpaced", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distH") },
        { label: t("app.distHCentered"), actionId: "arrange.distHCentered", hint: hintFor("arrange.distHCentered", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distHc") },
        { label: t("app.distVSpaced"), actionId: "arrange.distVSpaced", hint: hintFor("arrange.distVSpaced", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distV") },
        { label: t("app.distVCentered"), actionId: "arrange.distVCentered", hint: hintFor("arrange.distVCentered", prefHotkeys), disabled: nUnits < 3, action: () => arrangeRef.current("distVc") },
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
    arrangeExtra: [...distributeItems, ...designUi.arrangeItems, ...jigsUi.arrangeItems],
    // Guides belong to the project (not the document's name, which projects can share); wait until both are known.
    guideScope: doc && engine.projectKnown ? guideScope(file.projectName, file.openedName) : null,
  });
  // Text and Barcode tools, text/barcode properties, the Variable Text window (text/useTextTools.tsx).
  const text = useTextTools({ doc, selection, selectIds, tool, setTool, activeLayer, design: workspace === "design", has, run, notify, report });
  // Fill Region (paint bucket): a closed shape for the area around a click, on a chosen layer (src/fill/).
  const fillTool = useFillRegionTool({
    doc,
    tool,
    setTool,
    activeLayer,
    design: workspace === "design",
    has,
    run,
    notify,
    selectIds,
    onStart: () => toolsUi.stopTabTool(),
  });

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
            refused.push({ name: imp.file.name, reason: errorText(e, docRef.current) });
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
      // Ctrl/Cmd with = + - 0 never zooms the page (the canvas has its own zoom keys).
      if (mod && !e.altKey && (k === "=" || k === "+" || k === "-" || k === "0")) e.preventDefault();
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
      if (dialog || fileModalOpen || filesUi.modalOpen || libsUi.modalOpen || toolsUi.modalOpen || designRef.current.modalOpen || jigsUi.modalOpen || appModeDialogOpen()) return;
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
    toolsUi.modalOpen,
    jigsUi.modalOpen,
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
    { label: t("app.cut"), actionId: "edit.cut", hint: "Ctrl+X", disabled: !hasSel, action: () => copy(true) },
    { label: t("app.copy"), actionId: "edit.copy", hint: "Ctrl+C", disabled: !hasSel, action: () => copy(false) },
    { label: t("app.paste"), actionId: "edit.paste", hint: "Ctrl+V", disabled: !designUi.canPaste, action: () => paste(false) },
    { label: t("app.duplicate"), actionId: "edit.duplicate", hint: "Ctrl+D", disabled: !hasSel || !has(C.OPT.duplicate), action: duplicate },
    { label: t("app.delete"), actionId: "edit.delete", hint: "Del", disabled: !hasSel, action: deleteSelection },
    { sep: true },
    { label: t("app.group"), actionId: "arrange.group", hint: "Ctrl+G", disabled: units < 2 || !has(C.OPT.groupCreate), action: group },
    { label: t("app.ungroup"), actionId: "arrange.ungroup", hint: "Ctrl+U", disabled: !grouped || !has(C.OPT.groupUngroup), action: ungroup },
    { sep: true },
    ...edit.menuItems,
    ...filesUi.contextItems,
    ...libsUi.menuItems,
    ...materialsUi.menuItems,
    ...designUi.menuItems,
    ...jigsUi.menuItems,
    ...(text.menuItems.length ? [{ sep: true } as const, ...text.menuItems] : []),
    ...tpl.menuItems,
    { sep: true },
    { label: t("app.alignLeft"), actionId: "arrange.alignLeft", hint: "L", disabled: units < 2, action: () => alignTo("left") },
    { label: t("app.alignHCenter"), actionId: "arrange.alignHCenter", hint: "C", disabled: units < 2, action: () => alignTo("center") },
    { label: t("app.alignRight"), actionId: "arrange.alignRight", hint: "R", disabled: units < 2, action: () => alignTo("right") },
    { label: t("app.alignTop"), actionId: "arrange.alignTop", hint: "T", disabled: units < 2, action: () => alignTo(undefined, "top") },
    { label: t("app.alignVCenter"), actionId: "arrange.alignVCenter", hint: "E", disabled: units < 2, action: () => alignTo(undefined, "center") },
    { label: t("app.alignBottom"), actionId: "arrange.alignBottom", hint: "B", disabled: units < 2, action: () => alignTo(undefined, "bottom") },
    { label: t("app.centreStock"), disabled: !hasSel || !doc?.stock, action: () => centreOn("stock") },
    { label: t("app.centreField"), actionId: "arrange.centerField", hint: "P", disabled: !hasSel, action: () => centreOn("field") },
    { sep: true },
    ...docking.menuItems,
    { sep: true },
    { label: t("app.flipH"), actionId: "arrange.flipH", hint: "Ctrl+Shift+H", disabled: !hasSel, action: () => flip("x") },
    { label: t("app.flipV"), actionId: "arrange.flipV", hint: "Ctrl+Shift+V", disabled: !hasSel, action: () => flip("y") },
    { sep: true },
    { label: t("app.selectAll"), actionId: "edit.selectAll", hint: "Ctrl+A", disabled: !doc, action: () => doc && setSelection(selectAll(doc)) },
    { label: t("app.invertSelection"), actionId: "edit.invert", hint: "Ctrl+Shift+I", disabled: !doc, action: () => doc && setSelection(invertSelection(selection, doc)) },
  ];

  // The framing hotkeys: the Laser window's framing options (components/FramingOptions.tsx), on the selection when
  // there is one, else the whole job; `style` overrides the frame style.
  const frameWith = (style?: "bounds" | "outline") =>
    void machineAction(frameWithOptions(selection.objects, { selectionOnly: selection.objects.length > 0, ...(style ? { style } : {}) }), t("app.framing"));
  // Request run / Start (Alt+S) goes through the one Start flow (laser/startFlow.ts): the same gates as the Start
  // button, so with the Job Checklist on and unticked it opens the checklist instead of firing, and in Design it opens
  // the Laser window first. A run still needs PIN approval, or without a PIN (`--approval click`) Start burns at once.
  const requestRun = () => void startFromHotkey();
  const laserDisabled = designOnlyReason(device);
  // "Open the Laser window" for the Start flow (checklist, Start from Design): the Laser workspace with its right side
  // shown (pinned, or its overlay on a rail); on the phone the Job sheet (workspace/PhoneShell.tsx listens).
  const openLaser = () => {
    if (layout === "phone") {
      window.dispatchEvent(new CustomEvent(OPEN_LASER_EVENT));
      return;
    }
    if (workspace !== "laser") switchWorkspace("laser");
    if (dockLayout.areaMode.right === "hidden") updateLayout((l) => setAreaMode(l, "right", null));
    setFocus(false);
    setRightOpen(true);
    if (policy.right === "rail") setOverlay("right");
  };
  // Everything the Start buttons, the status chip, the connection banner and the empty card need from the app.
  useLaserBridge({
    machine,
    link: engine.link,
    httpError: engine.httpError,
    designOnly: laserDisabled,
    workspace,
    selection: selection.objects,
    revision: doc?.revision ?? null,
    docEmpty: doc ? doc.objects.length === 0 : null,
    openLaser,
    notify,
    report,
    refreshMachine: () => void engine.refreshMachine(),
  });
  // Workspace hotkeys (workspace/hotkeys.ts); App's own keys stay in the handler above.
  ws.keymap.current = {
    blocked: () => !!dialog || fileModalOpen || devicesOpen || filesUi.modalOpen || libsUi.modalOpen || toolsUi.modalOpen || designUi.modalOpen || jigsUi.modalOpen || materialsUi.modalOpen || tpl.modalOpen || appModeDialogOpen(),
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
      "laser.relief3d": () => {
        if (workspace !== "laser") switchWorkspace("laser");
        setShowPreview(true);
        openRelief();
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
      // Ctrl+. (Cmd+.) stops only while a run is under way; otherwise it stays LightBurn's rotate key.
      "laser.stop": (chord) => (stopChordActs(chord, machine?.running === true || activeRuns(machine).length > 0) ? stop() : false),
      ...text.handlers,
      ...filesUi.handlers,
      ...libsUi.handlers,
      ...prodUi.handlers,
      ...materialsUi.handlers,
      ...designUi.handlers,
      ...jigsUi.handlers,
      ...edit.handlers,
      ...fillTool.handlers,
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
        const open = !(leftOpen || rightOpen) || focus;
        setFocus(false);
        setLeftOpen(open);
        setRightOpen(open);
      },
      "window.newTab": () => (tabs.api.supported ? tabs.api.newTab() : false),
      // Focus canvas: the docks, the palette and the bottom row go; the top bar (STOP) and the context strip stay.
      "view.focusCanvas": () => (layout === "phone" ? false : toggleFocus()),
      // Full screen (Alt+Enter): not while a line or pen is being drawn (Alt+Enter also finishes those).
      "view.fullscreen": () => (!fullscreen.supported || tool === "line" || tool === "pen" ? false : fullscreen.toggle()),
      "help.useAsApp": () => openAppModeDialog(),
      // S: command search (P2 turns the Commands dialog into it); in node editing plain S makes a node smooth.
      // (components/CommandSearch.tsx, hosted by the toolbar; it declines while no toolbar is mounted).
      "app.commandSearch": () => (tool === "node" || tool === "text" || !openCommandSearch() ? false : undefined),
    },
  };

  function toggleFocus() {
    setFocus((f) => !f);
    setOverlay(null);
  }

  // The window title follows the document and its unsaved state ("Plate • — OpenEtch").
  const titleName = file.projectName ?? file.openedName ?? doc?.name ?? null;
  useEffect(() => {
    document.title = windowTitle(titleName, file.dirty);
  }, [titleName, file.dirty]);
  // The browser's and the app window's chrome colour follows the top bar, which follows the theme (app/appMode.ts).
  useEffect(() => {
    syncThemeColor();
    if (typeof MutationObserver !== "function") return;
    const mo = new MutationObserver(() => syncThemeColor());
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, [layout]);

  // The touch HUD's selection action bar (touch/TouchHud.tsx).
  const touchAction = (id: string) => {
    switch (id) {
      case "copy":
        return copy(false);
      case "cut":
        return copy(true);
      case "paste":
        return paste(false);
      case "duplicate":
        return duplicate();
      case "delete":
        return deleteSelection();
      case "group":
        return group();
      case "ungroup":
        return ungroup();
      case "rotate90":
        if (selection.objects.length) void exec(C.transform(selection.objects, { rotate_deg: 90, about: "center" }));
        return;
      case "flipH":
        return flip("x");
      case "flipV":
        return flip("y");
      case "menu":
        return setMenu({ x: Math.round(window.innerWidth / 2), y: Math.round(window.innerHeight / 2) });
    }
  };

  const onSelectObject = (id: string, op: SelectOp | boolean) => {
    if (!doc) return;
    const o: SelectOp = typeof op === "boolean" ? (op ? "toggle" : "replace") : op;
    setSelection(applySelect(selection, doc, [id], o));
  };

  // ---- the shell's parts (placed per layout mode below) ----------------------------------------------------------

  const fileMenuFor = (label?: string) => (
    <FileMenu
      label={label}
      file={file}
      onImport={openImport}
      onDevices={canDevices && desktopFeatures(layout) ? () => setDevicesOpen(true) : undefined}
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
              data-action-id="window.newTab"
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
          {tpl.fileItems(close)}
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
      onSaveRd={
        isRuidaMachine(machine)
          ? () =>
              void jobApi
                .exportRd()
                .then(({ blob, filename }) => {
                  const name = rdFileName(filename, file.projectName ?? file.openedName ?? doc?.name ?? null);
                  saveBlob(blob, name);
                  notify(t("fm.rdSaved", { name }));
                })
                .catch(report)
          : undefined
      }
    />
  );

  const fileMenu = fileMenuFor();
  // The phone's More > File section has New / Open / Save rows of its own; the full menu under them gets another name.
  const phoneFileMenu = layout === "phone" ? fileMenuFor(t("shell.moreFileAll")) : null;

  const topBar = (onMenu?: () => void) => (
    <TopBar
      fileMenu={fileMenu}
      onMenu={onMenu}
      extra={
        <>
          {layout === "wide" && <WindowMenu focusCanvas={focus} onFocusCanvas={toggleFocus} />}
          <SimPinBadge machine={machine} />
          <ShutdownBadge machine={machine} />
          <TopActions
            machine={machine}
            onHotkeys={() => ws.open("hotkeys")}
            onPreferences={() => ws.open("prefs")}
            helpExtra={libsUi.helpItems}
            overflow={layout !== "wide"}
            windowItems={(close) => <WindowMenuEntries close={close} focusCanvas={focus} onFocusCanvas={toggleFocus} />}
          />
        </>
      }
      workspace={workspace}
      onWorkspace={switchWorkspace}
      onUndo={() => void undo()}
      onRedo={() => void redo()}
      onStop={stop}
      statusExtra={prodUi.chip(layout === "phone")}
      link={engine.link}
      httpError={engine.httpError}
      machine={machine}
      missingCommands={missingCommands}
      docName={file.openedName ?? doc?.name ?? null}
      projectName={file.projectName}
      dirty={file.dirty}
      onSaveAs={() => file.setModal({ kind: "saveAs" })}
      pendingRuns={runs.length}
      docExtra={tabs.addButton}
    />
  );

  const toolbar = (
    <Toolbar
      workspace={workspace}
      tool={tool}
      hasSelection={hasSel}
      units={units}
      grouped={grouped}
      hasStock={!!doc?.stock}
      showPreview={showPreview}
      leftOpen={leftMode === "pinned" || overlay === "left"}
      rightOpen={rightMode === "pinned" || overlay === "right"}
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
      designOnly={designOnlyReason(device)}
      onToggleLeft={() => toggleSide("left")}
      onToggleRight={() => toggleSide("right")}
      extra={
        <>
          {text.toolbar}
          {edit.toolbar}
          {fillTool.toolbar}
          {designUi.toolbar}
          {libsUi.toolbar}
          {materialsUi.toolbar}
          {toolsUi.toolbar}
          {jigsUi.toolbar}
        </>
      }
    />
  );

  const leftContent = (
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
  );

  // A GRBL gantry's Move and Console windows and a Ruida gantry's File List (LightBurn's), in both workspaces' Window
  // menus as in LightBurn; galvos have none of them.
  const gantryDocks = (
    <>
      {isGrblMachine(machine) && (
        <>
          <Dock id="move" title={t("win.move")}>
            <GrblMove notify={notify} report={report} />
          </Dock>
          {/* The console is a desktop feature (workspace/desktopOnly.ts): not in the phone's Job sheet. */}
          {desktopFeatures(layout) && (
            <Dock id="console" title={t("win.console")}>
              <GrblConsole notify={notify} report={report} />
            </Dock>
          )}
        </>
      )}
      {/* Stored files, Send, Start (through approval). */}
      {isRuidaMachine(machine) && (
        <Dock id="files" title={t("win.files")}>
          <RuidaFiles notify={notify} projectName={file.projectName ?? file.openedName ?? doc?.name ?? null} selection={selection.objects} />
        </Dock>
      )}
    </>
  );

  const rightContent = (
    <>
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
        {gantryDocks}
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
                  // Device editing (and its lens wizard) is a desktop feature: on the phone it is under More.
                  onOpenWindow={desktopFeatures(layout) ? () => setDevicesOpen(true) : undefined}
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
            extras={<LaserExtras designOnly={laserDisabled} galvo={!isGrblMachine(machine) && !isRuidaMachine(machine)} tools={libsUi.laserTools} notify={notify} report={report} />}
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
          {prodUi.dock}
          {gantryDocks}
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
          {selectedLayer && <LayerSection layer={selectedLayer} machine={machine} run={run} report={report} has={has} hasImages={layerHasImages(doc, selectedLayer.id)} />}
          </Dock>
        </div>
      )}
    </>
  );

  const toastButtons = toasts.map((x) => (
    <button
      type="button"
      key={x.id}
      className={`toast toast-${x.kind}`}
      role={x.kind === "error" ? "alert" : "status"}
      title={t("shell.dismiss")}
      onClick={() => setToasts((all) => all.filter((y) => y.id !== x.id))}
    >
      {x.msg}
    </button>
  ));

  const emptyDoc = !!doc && doc.objects.length === 0 && workspace === "design";
  const canvasStage = (
    <div className="center-stage">
      <Viewport
        doc={materialsUi.viewDoc}
        workspaceMm={workspaceMm}
        galvo={device ? isGalvo(device.kind) : undefined}
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
          withLayerFlash(jigsUi.wrap(designUi.wrap(toolsUi.wrap(libsUi.wrap(text.wrap(fillTool.wrap(edit.canvas)))))), doc, flash),
          positioning,
          (pt) =>
            void machineAction(jobApi.pointer(roundMm(pt[0]), roundMm(pt[1])), t("poslaser.done", { x: roundMm(pt[0]), y: roundMm(pt[1]) })),
          (end) => (
            <HudItem id="poslaser-bar" slot="tc" rank={25} minW={200}>
              <div className="vp-overlay lx-sp-bar" role="status">
                <span>{t("poslaser.bar")}</span>
                <button className="btn small" onClick={end}>
                  {t("common.cancel")}
                </button>
              </div>
            </HudItem>
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
      {emptyDoc && (
        <EmptyCanvasCard
          onImport={openImport}
          onOpen={() => file.setModal({ kind: "open" })}
          onDraw={(d) => setTool(d === "rect" ? "rect" : "text")}
          onTemplates={filesUi.openTemplates}
          onExamples={has("example.list") ? tpl.openExamples : undefined}
        />
      )}
      {tpl.hud}
      {/* Always mounted: it shows itself on touch, or everywhere with the selection bar set to Always. */}
      <TouchHud tool={tool} selectionCount={selection.objects.length} onAction={touchAction} />
      {/* A run waiting at a layer for the operator: Continue over the canvas (src/production/). */}
      {prodUi.hud}
      {/* One layout for everything that floats over the canvas (hud/CanvasHud.tsx). */}
      <CanvasHud />
    </div>
  );

  // The context strip (Fusion): the selection's X / Y / W / H, a tool's option bar (ContextStripPortal), or the hint.
  const contextStrip = (
    <div role="group" className={`context-strip${stripBusy ? " busy" : ""}${workspace !== "design" && !stripBusy ? " idle" : ""}`} aria-label={t("shell.contextStrip")}>
      <ContextStripHost />
      {!stripBusy && workspace === "design" && (selection.objects.length > 0 ? (
        <NumericBar doc={doc} selection={selection} run={run} report={report} />
      ) : (
        <span className="cs-hint">{tool === "select" ? t("shell.hintSelect") : t("shell.hintTool")}</span>
      ))}
    </div>
  );

  const palette = workspace === "design" && <PaletteBar doc={doc} selected={selection.objects} run={run} report={report} notify={notify} has={has} />;

  const timeline = (
    <Timeline
      timeline={engine.timeline}
      flash={engine.remoteEdit}
      onGoto={(marker) => void engine.historyGoto(marker).catch(report)}
      onDelete={(index) => void engine.historyDelete(index).catch(report)}
    />
  );
  const statusBar = (
    <StatusBar
      doc={doc}
      selection={selection}
      snap={prefs.snap}
      machineText={statusMachineText(machineState(machine), t)}
      estimateS={workspace === "design" ? estimateOf(engine.previewSummary) : null}
    />
  );

  const overlays = (
    <>
      <input ref={fileRef} type="file" accept={pickerAccept(importAccept(has("art.import_pdf") ? ENGINE_ACCEPT : null))} multiple hidden onChange={onPick} />

      {dialog === "stock" && <StockDialog doc={doc} run={run} onClose={closeDialog} />}
      {dialog === "place" && <PlaceDialog ids={selection.objects} run={run} onClose={closeDialog} />}
      {dialog === "commands" && <CommandPalette commands={engine.commands} run={run} onClose={closeDialog} />}
      {moveTogether && <MoveTogetherDialog ids={selection.objects} axis={moveTogether} run={run} onClose={() => setMoveTogether(null)} />}
      {edit.dialogs}
      {filesUi.dialogs}
      {libsUi.dialogs}
      <ReliefHost preview={engine.preview} doc={doc} active={previewOn} />
      {prodUi.dialogs}
      {materialsUi.dialogs}
      {materialsUi.hud}
      {toolsUi.dialogs}
      {jigsUi.dialogs}
      {designUi.dialogs}
      {text.dialogs}
      {tpl.dialogs}
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
      {layout !== "phone" && <FloatHost />}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={forLayout(menuItems, layout)} onClose={() => setMenu(null)} />}
      {dropLabel && <DropOverlay label={dropLabel} />}
      {/* While a dialog (or a full-height sheet) is open the toasts are a strip inside it, so none covers it. */}
      {toastDialog && toasts.length > 0 && createPortal(<div className="toasts-dialog">{toastButtons}</div>, toastDialog)}
      <div className="toasts" ref={toastsRef}>
        {!toastDialog && toastButtons}
      </div>
      {about && <AboutDialog machine={machine} onClose={() => setAbout(false)} />}
    </>
  );

  // The production cockpit is a mode of the app: nothing else is shown (its own STOP; the app's toasts and dialogs).
  if (prodUi.cockpitOpen) return prodUi.renderCockpit(overlays);

  if (layout === "phone") {
    const themeName = { light: t("theme.light"), dark: t("theme.dark"), system: t("theme.system") }[themePref];
    const nextTheme = ({ light: "dark", dark: "system", system: "light" } as const)[themePref];
    const moreRow = (label: string, action: () => void, disabled = false) => (
      <button type="button" className="menu-item" disabled={disabled} onClick={action}>
        <span>{label}</span>
      </button>
    );
    return (
      <PhoneShell
        className={`ws-${workspace}`}
        topBar={(openMenu) => topBar(openMenu)}
        banner={<ConnectionBanner />}
        canvas={canvasStage}
        canvasHud={
          <>
            <button type="button" className="icon-btn phone-hud-btn" title={t("top.undo")} aria-label={t("top.undo")} onClick={() => void undo()}>
              <Icon name="undo" />
            </button>
            <button type="button" className="icon-btn phone-hud-btn" title={t("top.redo")} aria-label={t("top.redo")} onClick={() => void redo()}>
              <Icon name="redo" />
            </button>
          </>
        }
        toolStrip={workspace === "design" ? toolbar : null}
        selectionSheet={
          workspace === "design" && hasSel ? (
            <div className="phone-selection">
              <NumericBar doc={doc} selection={selection} run={run} report={report} />
              <div className="phone-actions">
                <button type="button" className="btn" disabled={!doc?.stock} onClick={() => centreOn("stock")}>
                  {t("app.centreStock")}
                </button>
                <button type="button" className="btn" onClick={() => centreOn("field")}>
                  {t("app.centreField")}
                </button>
                {jigsUi.phoneButtons}
                <button type="button" className="btn" onClick={() => touchAction("rotate90")}>
                  {t("shell.rotate90")}
                </button>
                <button type="button" className="btn btn-danger" onClick={deleteSelection}>
                  {t("app.delete")}
                </button>
              </div>
            </div>
          ) : null
        }
        layersSheet={
          <>
            {doc && (
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
            )}
            <div className="phone-palette">
              <PaletteBar doc={doc} selected={selection.objects} run={run} report={report} notify={notify} has={has} />
            </div>
            {/* The Material Library (finishes: photos from the phone camera, Palette, Warmer / Cooler, bracket test). */}
            {libsUi.dock}
          </>
        }
        jobSheet={workspace === "laser" ? rightContent : null}
        // Start (with the Job Checklist when there is no PIN) pinned under the Job sheet's header: in view at every
        // sheet height. LaserDock leaves its own Start row out on the phone; STOP stays in the top bar.
        jobPinned={workspace === "laser" ? <StartButton variant="sheet" /> : null}
        onTab={(tab) => {
          const w: Workspace = tab === "job" ? "laser" : "design";
          if (w !== workspace) switchWorkspace(w);
        }}
        moreItems={(closeMore) => (
          <>
            {/* Command search lives in the Canvas sheet's tool strip; this opens it from here (PhoneShell). */}
            <div className="phone-more-body phone-more-search">
              {moreRow(`${t("rb.searchTitle")}…`, () => window.dispatchEvent(new CustomEvent(PHONE_SEARCH_EVENT)))}
            </div>
            <details open>
              <summary>
                <Icon name="file" />
                {t("shell.moreFile")}
              </summary>
              <div className="phone-more-body">
                {moreRow(t("fm.new"), () => file.newDocument(), file.busy)}
                {moreRow(t("fm.open"), () => file.setModal({ kind: "open" }), file.busy)}
                {moreRow(t("fm.save"), () => saveFile(), file.busy)}
                {moreRow(t("fm.saveAs"), () => file.setModal({ kind: "saveAs" }), file.busy)}
                {moreRow(t("fm.import"), openImport, file.busy)}
                {/* The full File menu (Export, Save RD / G-code, templates...), named so it does not read "File" twice. */}
                <div className="phone-filemenu">{phoneFileMenu}</div>
              </div>
            </details>
            <details>
              <summary>
                <Icon name="layers" />
                {t("win.browser")}
              </summary>
              <div className="phone-more-body">{leftContent}</div>
            </details>
            <details>
              <summary>
                <Icon name="history" />
                {t("shell.history")}
              </summary>
              <div className="phone-more-body">
                <Timeline
                  variant="list"
                  timeline={engine.timeline}
                  flash={engine.remoteEdit}
                  onGoto={(marker) => void engine.historyGoto(marker).catch(report)}
                  onDelete={(index) => void engine.historyDelete(index).catch(report)}
                />
              </div>
            </details>
            <details>
              <summary>
                <Icon name="settings" />
                {t("shell.moreSettings")}
              </summary>
              <div className="phone-more-body">
                {moreRow(t("prefs.title"), () => ws.open("prefs"))}
                {moreRow(t("theme.switchTitle", { theme: themeName }), () => updatePrefs({ theme: nextTheme }))}
              </div>
            </details>
            <details>
              <summary>
                <Icon name="more" />
                {t("help.menu")}
              </summary>
              <div className="phone-more-body">
                <HelpItems close={() => {}} onHotkeys={() => ws.open("hotkeys")} onAbout={() => setAbout(true)} helpExtra={libsUi.helpItems} />
              </div>
            </details>
            <details>
              <summary>
                <Icon name="monitor" />
                {t("shell.desktopFeatures")}
              </summary>
              {/* Left out of the phone's strip and menus (workspace/desktopOnly.ts); the ones a phone can still run open
                  from here, the tools close the sheet so the canvas is in view. */}
              <p className="phone-note hint">{t("shell.desktopFeaturesHint")}</p>
              <div className="phone-more-body">
                {moreRow(t("em.editNodes"), () => (closeMore(), setTool("node")), !hasSel)}
                {moreRow(t("em.pen"), () => (closeMore(), setTool("pen")))}
                {moreRow(t("em.polygon"), () => (closeMore(), setTool("polygon")))}
                {moreRow(t("em.trim"), () => (closeMore(), setTool("trim")))}
                {moreRow(t("et.weld"), () => (closeMore(), runHotkeyAction("tool.weld")), !hasSel)}
                {moreRow(toolsUi.tabToolActive ? t("tools.stopTabs") : t("tools.addTabs"), () => (closeMore(), toolsUi.toggleTabTool()))}
                {toolsUi.openUserTools && moreRow(t("ut.manage"), toolsUi.openUserTools)}
                {canDevices && moreRow(t("shell.dfDevicesOpen"), () => setDevicesOpen(true))}
              </div>
              <div className="phone-note hint">
                {t("shell.dfElsewhere")}
                <ul>
                  <li>{t("shell.dfDocking")}</li>
                  <li>{t("shell.dfArt")}</li>
                  {isGrblMachine(machine) && <li>{t("shell.dfConsole")}</li>}
                </ul>
              </div>
            </details>
          </>
        )}
        overlays={overlays}
      />
    );
  }

  const sizeVars = {
    "--dock-left-size": `${dockLayout.sizes.left}px`,
    "--dock-right-size": `${dockLayout.sizes.right}px`,
    "--dock-bottom-size": `${dockLayout.sizes.bottom}px`,
  } as CSSProperties;
  // Tablet: a selection's settings open on request, from a pill over the canvas (selecting never opens a drawer).
  const propertiesPill = layout === "tablet" && workspace === "design" && rightMode === "rail" && overlay !== "right" && (selection.objects.length > 0 || selection.stock) && (
    <HudItem id="properties-pill" slot="br" rank={12}>
      <button className="btn properties-pill" title={t("shell.propertiesPillTitle")} onClick={() => setOverlay("right")}>
        <Icon name="sliders" size={16} />
        {t("shell.propertiesPill")}
      </button>
    </HudItem>
  );

  return (
    <div className={`app ws-${workspace}`} style={sizeVars} data-focus={focus ? "" : undefined}>
      {topBar()}
      <ConnectionBanner />
      {tabs.bar && <div className="doc-tabs-row">{tabs.bar}</div>}
      {layout !== "tablet" && toolbar}
      <div className="main">
        {layout === "tablet" && <div className="tool-rail">{toolbar}</div>}
        <SidePanel
          side="left"
          mode={leftMode}
          overlay={overlay === "left"}
          onOverlay={setLeftOverlay}
          label={t("win.browser")}
          // Tablet: the tool rail's Browser button opens the left side, so it needs no rail of its own (the canvas keeps
          // 900 px at 1024 wide and 700 at 820).
          railVisible={layout !== "tablet"}
        >
          <DockAreaHost area="left" />
          {leftContent}
        </SidePanel>

        <section className="center">
          {contextStrip}
          {canvasStage}
          {propertiesPill}
          {bottomHasWindows && !focus && <Splitter area="bottom" label={t("shell.resize", { name: t("shell.bottomStrip") })} />}
          <DockAreaHost area="bottom" />
          {!focus && palette}
        </section>

        <SidePanel
          side="right"
          mode={rightMode}
          overlay={overlay === "right"}
          onOverlay={setRightOverlay}
          // The area's name, not one of its windows' (Cuts / Layers, the libraries, Shape Properties; Laser, Machine...).
          label={workspace === "laser" ? t("shell.areaLaser") : t("shell.areaDesign")}
        >
          <DockAreaHost area="right" />
          {rightContent}
        </SidePanel>
      </div>
      <div className="bottom-row" data-bottom={row}>
        {timeline}
        {layout === "compact" && (
          <button
            className={`icon-btn small tl-pin${timelinePinned ? " active" : ""}`}
            aria-pressed={timelinePinned}
            title={timelinePinned ? t("shell.timelineUnpin") : t("shell.timelinePin")}
            aria-label={t("shell.timelinePin")}
            onClick={() => updatePrefs({ timelinePinned: !timelinePinned })}
          >
            <Icon name="pin" size={16} />
          </button>
        )}
        {statusBar}
      </div>
      {overlays}
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
