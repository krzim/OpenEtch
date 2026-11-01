// Application shell: top bar, toolbar, Browser | Viewport | Properties, Timeline. On narrow screens (iPad portrait,
// phones) the side panels become drawers over the viewport. All document state comes from the engine; the only
// client state is the view: workspace, tool, selection, clipboard, open panels and dialogs, display options.
//
// Files dropped anywhere on the window (or picked with Import / Ctrl+I) are routed by api/drop.ts: SVG, DXF and bitmaps
// are imported into the document, a LightBurn .lbrn2 opens the Open/Import dialog, a .openetch opens the project.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "./api/client.ts";
import * as C from "./api/commands.ts";
import {
  IMPORT_ACCEPT,
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
import { MAX_UPLOAD_BYTES, commandResult, fileError, warningList } from "./api/project.ts";
import { pendingRuns, stopToast } from "./api/runs.ts";
import type { Vec2 } from "./api/types.ts";
import { useEngine } from "./api/useEngine.ts";
import { BrowserPanel } from "./components/BrowserPanel.tsx";
import { ContextMenu, type MenuItem } from "./components/ContextMenu.tsx";
import { ArrayDialog, CommandPalette, PlaceDialog, StockDialog } from "./components/Dialogs.tsx";
import { DropOverlay, useFileDrop } from "./components/FileDrop.tsx";
import { FileDialogs, FileMenu, useFileActions } from "./components/FileMenu.tsx";
import { MachineCard, PreviewPlayer, PreviewStatsCard, RunsCard } from "./components/LaserPanel.tsx";
import { LayerSection, PropertiesPanel } from "./components/PropertiesPanel.tsx";
import { Timeline } from "./components/Timeline.tsx";
import { Toolbar, type ArrangeAction } from "./components/Toolbar.tsx";
import { TopBar, type Workspace } from "./components/TopBar.tsx";
import { Viewport, type PreviewOptions, type SnapSettings, type Tool, type ViewStyle } from "./components/Viewport.tsx";
import { alignMoves, distributeMoves, nudgeDelta, rotateKeyDeg, roundMm, type AlignX, type AlignY } from "./editing.ts";
import { boxCenter, objectBounds, toPathData, geometryCmds, unionBoxes, type Box } from "./geometry.ts";
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

type Dialog = "stock" | "place" | "array" | "commands" | null;
interface Toast {
  id: number;
  msg: string;
  kind: "info" | "error";
}

/** Copied objects: ids to duplicate while they exist, plus outlines to recreate after a cut. */
interface Clip {
  ids: string[];
  box: Box | null;
  shapes: { layer: string; name: string; d: string; image: boolean }[];
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
  const [previewOptions, setPreviewOptions] = useState<PreviewOptions>({ hidden: new Set(), upTo: null, shade: true, jumps: true });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const narrow = useMediaQuery(NARROW_QUERY);
  const [leftOpen, setLeftOpen] = useState(!narrow);
  const [rightOpen, setRightOpen] = useState(!narrow);
  const fileRef = useRef<HTMLInputElement>(null);
  const toastId = useRef(0);
  const clip = useRef<Clip | null>(null);
  const cursor = useRef<Vec2 | null>(null);
  const importing = useRef(false);

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
  const fieldMm = machineFieldMm ?? DEFAULT_FIELD_MM;
  const file = useFileActions({ engine, machineFieldMm, fallbackFieldMm: DEFAULT_FIELD_MM, notify, report, onImported: selectIds });
  const fileModalOpen = file.modal !== null;
  const saveFile = file.save;
  const activeLayer = selection.layer ?? doc?.layers[0]?.id ?? null;
  const runs = pendingRuns(machine);
  const outside = useMemo(() => outsideField(doc, fieldMm), [doc, fieldMm]);

  const run = engine.run;
  const exec = useCallback((inv: Parameters<typeof run>[0]) => run(inv).catch(report), [run, report]);

  // ---- import: drop and picker share one router ------------------------------------------------------------------

  const importOne = useCallback(
    async (imp: DirectImport<File>, layer: string | null): Promise<string[]> => {
      const name = importName(imp.file.name);
      if (imp.file.size > inlineLimit(imp.kind)) {
        // Large files go up raw; the engine imports them on its own thread.
        const res = await api.upload(imp.file, imp.kind).catch((e: unknown) => Promise.reject(fileError(e, imp.file.name)));
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
        else notify("This dialog takes LightBurn .lbrn2 files.", "error");
        return;
      }
      // While another dialog is open or a file task runs, a drop is still kept from the browser but not imported.
      const blocked =
        importing.current || file.busy ? "An import is still running." : dialog || (m && m.kind !== "open") ? "Finish the open dialog first." : null;
      if (blocked) {
        notify(blocked, "error");
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
            const ids = await importOne(imp, activeLayer);
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
      if (plan.lbrn2.length) file.setModal({ kind: "import", files: plan.lbrn2, mode: "merge" });
      if (imported.length || refused.length) {
        const sum = dropSummary({ imported, objects: created.length, refused, pending: plan.lbrn2.length });
        notify(sum.msg, sum.kind);
      }
    },
    [file, dialog, has, notify, importOne, activeLayer, selectIds],
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
        notify("Select at least two objects to align. The last one selected stays put.", "error");
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
        notify("Define a stock first.", "error");
        return;
      }
      if (has(C.OPT.align)) {
        void exec(C.align(selection.objects, { reference: { kind: target }, x: "center", y: "center", mode: "group" }));
        return;
      }
      const b = boxOf(selection.objects);
      if (!b) return;
      const c = boxCenter(b);
      const t: Vec2 = target === "stock" && d.stock ? d.stock.center : [0, 0];
      void exec(C.transform(selection.objects, { translate: [roundMm(t[0] - c[0]), roundMm(t[1] - c[1])] }));
    },
    [selection.objects, has, exec, boxOf, notify],
  );

  const distribute = useCallback(
    (axis: "x" | "y", by: "centers" | "gaps") => {
      const d = docRef.current;
      if (!d) return;
      const units = selUnits();
      if (units.length < 3) {
        notify("Select at least three objects to distribute.", "error");
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
      notify("This engine cannot group objects yet.", "error");
      return;
    }
    const units = unitsOf(d, selection.objects);
    if (units.length < 2) {
      notify("Select at least two objects or groups to group.", "error");
      return;
    }
    void exec(C.groupCreate(units));
  }, [selection.objects, has, exec, notify]);

  const ungroup = useCallback(() => {
    const d = docRef.current;
    if (!d) return;
    const groups = [...new Set(selection.objects.map((id) => topGroup(d, id)).filter((g): g is string => g !== null))];
    if (groups.length === 0) {
      notify("The selection has no groups.", "error");
      return;
    }
    if (!has(C.OPT.groupUngroup)) {
      notify("This engine cannot ungroup yet.", "error");
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
      notify("This engine cannot duplicate objects.", "error");
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

  const copy = useCallback(
    (cut: boolean) => {
      const d = docRef.current;
      if (!d || selection.objects.length === 0) return;
      const set = new Set(selection.objects);
      const objs = d.objects.filter((o) => set.has(o.id));
      clip.current = {
        ids: objs.map((o) => o.id),
        box: boxOf(selection.objects),
        shapes: objs.map((o) => ({ layer: o.layer, name: o.name, d: toPathData(geometryCmds(o.geometry), o.transform), image: o.geometry.kind === "image" })),
      };
      if (cut) void exec(C.remove(selection.objects));
      notify(`${cut ? "Cut" : "Copied"} ${objs.length} object${objs.length === 1 ? "" : "s"}.`);
    },
    [selection.objects, boxOf, exec, notify],
  );

  /** Paste at the pointer when it is over the canvas, else in place. */
  const paste = useCallback(
    (inPlace: boolean) => {
      const d = docRef.current;
      const c = clip.current;
      if (!d || !c) return;
      const at = !inPlace && cursor.current && c.box ? cursor.current : null;
      const off: Vec2 = at && c.box ? [roundMm(at[0] - boxCenter(c.box)[0]), roundMm(at[1] - boxCenter(c.box)[1])] : [0, 0];
      const exist = c.ids.every((id) => d.objects.some((o) => o.id === id));
      if (exist && has(C.OPT.duplicate)) {
        run(C.duplicate(c.ids, off))
          .then((r) => selectIds(createdIds(r)))
          .catch(report);
        return;
      }
      if (!has(C.OPT.shapePath)) {
        notify("The copied objects no longer exist.", "error");
        return;
      }
      const shapes = c.shapes.filter((s) => !s.image && d.layers.some((l) => l.id === s.layer));
      if (shapes.length < c.shapes.length) notify("Images and objects on deleted layers cannot be pasted after a cut.", "error");
      if (!shapes.length) return;
      const invs = shapes.map((s) => C.shapePath(s.d, s.layer, s.name));
      const moveAfter = off[0] !== 0 || off[1] !== 0;
      run(invs.length === 1 ? invs[0] : invs)
        .then(async (r) => {
          const results = r && typeof r === "object" && Array.isArray((r as { results?: unknown }).results) ? (r as { results: unknown[] }).results : [r];
          const ids = results.flatMap((x) => createdIds(x));
          if (moveAfter && ids.length) await run(C.transform(ids, { translate: off }));
          selectIds(ids);
        })
        .catch(report);
    },
    [has, run, selectIds, report, notify],
  );

  // Arrow-key nudges merge while a command is in flight, so holding a key sends a few commands, not dozens.
  const nudgeQ = useRef<{ ids: string[]; stock: boolean; d: Vec2 } | null>(null);
  const nudgeBusy = useRef(false);
  const flushNudge = useCallback(() => {
    const q = nudgeQ.current;
    if (nudgeBusy.current || !q) return;
    nudgeQ.current = null;
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
    (d: Vec2) => {
      const stock = selection.objects.length === 0 && selection.stock;
      if (!stock && selection.objects.length === 0) return;
      if (stock && !has(C.OPT.stockMove)) return;
      const q = nudgeQ.current;
      const key = stock ? "stock" : selection.objects.join(",");
      if (q && (q.stock ? "stock" : q.ids.join(",")) === key) q.d = [q.d[0] + d[0], q.d[1] + d[1]];
      else {
        flushNudge();
        nudgeQ.current = { ids: selection.objects, stock, d };
      }
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
      if (dialog || fileModalOpen) return;
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
              if (selection.objects.length && window.getSelection()?.isCollapsed !== false) {
                e.preventDefault();
                copy(false);
              }
              return;
            case "x":
              if (selection.objects.length) {
                e.preventDefault();
                copy(true);
              }
              return;
            case "v":
              if (clip.current) {
                e.preventDefault();
                paste(false);
              }
              return;
          }
        }
      }
      if (e.key === "Escape") {
        setMenu(null);
        if (tool !== "select") setTool("select");
        else setSelectionRaw(EMPTY_SELECTION);
        return;
      }
      if (!design) return;
      if (e.altKey && !mod && k === "v" && clip.current) {
        e.preventDefault();
        paste(true);
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
      const nd = !e.altKey ? nudgeDelta(e.key, m) : null;
      if (nd) {
        if (selection.objects.length || selection.stock) {
          e.preventDefault();
          nudge(nd);
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
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    dialog,
    fileModalOpen,
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

  // LightBurn: Alt+Shift+W toggles the filled view.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey && e.shiftKey && e.key.toLowerCase() === "w" && !typingIn(e.target)) {
        e.preventDefault();
        setPrefs((p) => ({ ...p, viewStyle: p.viewStyle === "filled" ? "wireframe" : "filled" }));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
  const selectedLayer = selection.layer ? doc?.layers.find((l) => l.id === selection.layer) : undefined;
  const hasSel = selection.objects.length > 0;
  const units = doc ? unitsOf(doc, selection.objects).length : 0;
  const grouped = doc ? anyGrouped(doc, selection.objects) : false;

  const menuItems: MenuItem[] = [
    { label: "Cut", hint: "Ctrl+X", disabled: !hasSel, action: () => copy(true) },
    { label: "Copy", hint: "Ctrl+C", disabled: !hasSel, action: () => copy(false) },
    { label: "Paste", hint: "Ctrl+V", disabled: !clip.current, action: () => paste(false) },
    { label: "Duplicate", hint: "Ctrl+D", disabled: !hasSel || !has(C.OPT.duplicate), action: duplicate },
    { label: "Delete", hint: "Del", disabled: !hasSel, action: deleteSelection },
    { sep: true },
    { label: "Group", hint: "Ctrl+G", disabled: units < 2 || !has(C.OPT.groupCreate), action: group },
    { label: "Ungroup", hint: "Ctrl+U", disabled: !grouped || !has(C.OPT.groupUngroup), action: ungroup },
    { sep: true },
    { label: "Align left", hint: "L", disabled: units < 2, action: () => alignTo("left") },
    { label: "Align horizontal centres", hint: "C", disabled: units < 2, action: () => alignTo("center") },
    { label: "Align right", hint: "R", disabled: units < 2, action: () => alignTo("right") },
    { label: "Align top", hint: "T", disabled: units < 2, action: () => alignTo(undefined, "top") },
    { label: "Align vertical centres", hint: "E", disabled: units < 2, action: () => alignTo(undefined, "center") },
    { label: "Align bottom", hint: "B", disabled: units < 2, action: () => alignTo(undefined, "bottom") },
    { label: "Centre on stock", disabled: !hasSel || !doc?.stock, action: () => centreOn("stock") },
    { label: "Centre on field", hint: "P", disabled: !hasSel, action: () => centreOn("field") },
    { sep: true },
    { label: "Flip horizontal", hint: "Ctrl+Shift+H", disabled: !hasSel, action: () => flip("x") },
    { label: "Flip vertical", hint: "Ctrl+Shift+V", disabled: !hasSel, action: () => flip("y") },
    { sep: true },
    { label: "Select all", hint: "Ctrl+A", disabled: !doc, action: () => doc && setSelection(selectAll(doc)) },
    { label: "Invert selection", hint: "Ctrl+Shift+I", disabled: !doc, action: () => doc && setSelection(invertSelection(selection, doc)) },
  ];

  const onSelectObject = (id: string, op: SelectOp | boolean) => {
    if (!doc) return;
    const o: SelectOp = typeof op === "boolean" ? (op ? "toggle" : "replace") : op;
    setSelection(applySelect(selection, doc, [id], o));
  };

  return (
    <div className={`app ws-${workspace}${narrow ? " narrow" : ""}`}>
      <TopBar
        fileMenu={<FileMenu file={file} onImport={openImport} />}
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
        onArray={() => setDialog("array")}
        onDelete={deleteSelection}
        onArrange={arrange}
        onSnap={(snap) => setPrefs((p) => ({ ...p, snap }))}
        onViewStyle={(viewStyle) => setPrefs((p) => ({ ...p, viewStyle }))}
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
              has={has}
              onSelectObject={onSelectObject}
              onSelectLayer={(id, all) => {
                if (all && doc) setSelection(selectLayerObjects(doc, id));
                else setSelection({ objects: [], layer: id, stock: false });
              }}
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
          />
          {previewOn && engine.preview && (
            <PreviewPlayer preview={engine.preview} options={previewOptions} onOptions={setPreviewOptions} />
          )}
        </section>

        <aside className={`panel panel-right${rightOpen ? " open" : ""}`} aria-label={workspace === "laser" ? "Machine" : "Properties"}>
          <div className="panel-title">{workspace === "laser" ? "Machine" : "Properties"}</div>
          <div className="panel-body">
            {workspace === "design" ? (
              <PropertiesPanel
                doc={doc}
                selection={selection}
                machine={machine}
                fieldMm={fieldMm}
                has={has}
                run={run}
                report={report}
                onEditStock={() => setDialog("stock")}
                onArrange={arrange}
              />
            ) : (
              <div className="props">
                <MachineCard machine={machine} httpError={engine.httpError} />
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
                {selectedLayer && <LayerSection layer={selectedLayer} machine={machine} run={run} report={report} />}
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

      <input ref={fileRef} type="file" accept={IMPORT_ACCEPT} multiple hidden onChange={onPick} />

      {dialog === "stock" && <StockDialog doc={doc} run={run} onClose={closeDialog} />}
      {dialog === "place" && <PlaceDialog ids={selection.objects} run={run} onClose={closeDialog} />}
      {dialog === "array" && doc && (
        <ArrayDialog doc={doc} ids={selection.objects} fieldMm={fieldMm} run={run} onClose={closeDialog} onCreated={(ids) => selectIds([...selection.objects, ...ids])} />
      )}
      {dialog === "commands" && <CommandPalette commands={engine.commands} run={run} onClose={closeDialog} />}
      <FileDialogs file={file} docName={doc?.name ?? null} />
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
