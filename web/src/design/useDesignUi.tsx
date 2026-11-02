// The design-engine layer of the UI, as one hook for App.tsx (LightBurn's Edit and Tools menus, Shape Properties, the
// Art Library window and the clipboard; engine commands in crates/commands/src/builtin/{design,construct,
// art_library}.rs):
// - Edit dropdown: Cut / Copy / Paste / Paste in place, Use system clipboard, Select open shapes, Select open shapes set
//   to fill, Select contained shapes, Curve tolerance.
// - Tools dropdown: Warp selection and Deform selection (handles on the canvas with a live preview; Shift keeps
//   horizontal and Alt vertical symmetry, double-click resets a handle, Enter applies as one `path.warp`, Esc
//   cancels), Generate tangent circle (candidates in light red, click one), Rubber-band outline, Convert toolpath to
//   path, Art Library.
// - Arrange additions: Move selected objects (nine page positions), Move horizontally / vertically together.
// - Shape Properties for a lone rectangle, ellipse or polygon; the Grid Array dialog; the Art Library dock with drag
//   and drop onto the canvas.
// - Clipboard (LightBurn's Clipboard tools): the internal copy is the selection as an Art Library fragment (objects,
//   groups, layers), so Paste after Cut brings back images, groups, text and parametric shapes; Paste (Ctrl+V) centres
//   it on the pointer, Paste in place (Alt+V) keeps its position, one batch of `art.insert` per top-level piece (one
//   undo step). With "Use system clipboard" (LightBurn's default) Copy also puts the selection on the system clipboard
//   as SVG (text, and image/svg+xml where the browser allows), and Paste takes what another program copied: SVG code
//   (Inkscape, Illustrator with "Include SVG Code"), a bitmap, or text (a text object); our own copy is recognised by
//   a marker and pasted from the internal copy. Keyboard copy and paste use the page's copy/paste events (they work
//   without a permission prompt and outside secure contexts); the menu entries use the async Clipboard API.

import { HudItem } from "../hud/CanvasHud.tsx";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { bytesToBase64 } from "../api/drop.ts";
import * as G from "../api/geomCommands.ts";
import type { GDocument, Vec2, VirtualArray } from "../api/types.ts";
import type { CanvasCtx, CanvasTool, RenderCtx } from "../canvasTool.ts";
import { handleHitPx, pointerClass, type PointerClass } from "../pointer.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import { Dropdown } from "../components/EditMenus.tsx";
import { RibbonSlot, usePublish } from "../components/Ribbon.tsx";
import { runHotkeyAction } from "../components/commands.ts";
import { Icon } from "../components/ui.tsx";
import { objectBounds, unionBoxes, type Box } from "../geometry.ts";
import { circleOf } from "../snapping.ts";
import { unitsOf, type Selection } from "../selection.ts";
import { loadTextDefaults, shapeText } from "../text/textModel.ts";
import type { KeyHandler } from "../workspace/useKeymap.ts";
import type { MsgKey } from "../workspace/i18n.ts";
import { tr } from "../workspace/tr.ts";
import { useT } from "../workspace/usePrefs.ts";
import { ART_DRAG_TYPE, ArtLibraryDock, openArtDock } from "./ArtLibrary.tsx";
import { DCMD, artApi, rubberBand, selectQuery, tangentCandidates, tangentCircle, toolpathToPath, warp } from "./designApi.ts";
import { CurveToleranceDialog, GridArrayDialog, MovePositionDialog, MoveTogetherDialog } from "./DesignDialogs.tsx";
import {
  classifyClipboard,
  fragmentBounds,
  fragmentOf,
  handlesUnchanged,
  identityHandles,
  moveHandle,
  nearestCandidate,
  parseCandidates,
  pasteInvocations,
  perspectiveProblem,
  selectionSvg,
  svgIn,
  warpParams,
  virtualArrayOf,
  warpPreview,
  type ArtFragment,
  type ClipContent,
  type TangentCandidate,
  type WarpKind,
} from "./model.ts";
import { ShapeProperties } from "./ShapeProperties.tsx";
import { SYSTEM_CLIPBOARD_EVENT } from "../files/LightBurnPrefsImport.tsx";
import { desktopFeatures, forLayout } from "../workspace/desktopOnly.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import "./design.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;
type Notify = (msg: string, kind?: "info" | "error") => void;

export interface DesignUiProps {
  doc: GDocument | null;
  selection: Selection;
  selectIds: (ids: string[]) => void;
  activeLayer: string | null;
  workspaceMm: Vec2;
  /** The Design workspace is shown. */
  design: boolean;
  /** Another dialog of the app is open: keys and the clipboard stay off. */
  blocked: boolean;
  has: (cmd: string) => boolean;
  run: Run;
  notify: Notify;
  report: (e: unknown) => void;
  /** The field point under the pointer while it is over the canvas, else null. */
  cursor: { current: Vec2 | null };
  /** Refetch the document (after an edit made through another route than /api/execute). */
  refreshDoc: () => void;
  /** More Tools dropdown entries (src/tools/: tabs, dogbone, Resize Slots, Multi-File Trace), after a separator. */
  extraTools?: MenuItem[];
}

type Dlg = { kind: "movePosition" } | { kind: "moveTogether"; axis: "x" | "y" } | { kind: "curveTolerance" } | { kind: "array"; edit?: VirtualArray };

interface WarpSession {
  kind: "warp";
  mode: WarpKind;
  ids: string[];
  box: Box;
  handles: Vec2[];
  drag: { index: number; base: Vec2[] } | null;
  hover: number | null;
}
interface TangentSession {
  kind: "tangent";
  ids: string[];
  cands: TangentCandidate[];
  hover: number | null;
}
type Session = WarpSession | TangentSession | null;

interface Clip {
  fragment: ArtFragment;
  marker: string;
  /** The copied objects' ids, for engines without art.insert (duplicate while they exist). */
  ids: string[];
}

const SYS_KEY = "openetch.systemClipboard";
const DBL_MS = 350;

function loadSysClip(): boolean {
  try {
    return localStorage.getItem(SYS_KEY) !== "off";
  } catch {
    return true;
  }
}

function typingIn(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/** Some text is selected on the page (copying it is the browser's job). */
function textSelected(): boolean {
  const s = window.getSelection?.();
  return !!s && !s.isCollapsed && s.toString().length > 0;
}

function newMarker(): string {
  const a = new Uint32Array(2);
  crypto.getRandomValues(a);
  return `${a[0].toString(36)}${a[1].toString(36)}`;
}

function store<T>(init: T) {
  let s = init;
  const subs = new Set<() => void>();
  return {
    get: () => s,
    set: (v: T) => {
      s = v;
      for (const f of subs) f();
    },
    sub: (f: () => void) => {
      subs.add(f);
      return () => {
        subs.delete(f);
      };
    },
  };
}

/** The box `[min, max]` an edit result reports, if any. */
function resultBox(r: unknown): Box | null {
  const b = (r as { bounds?: { min?: unknown; max?: unknown } } | null)?.bounds;
  if (!b || !Array.isArray(b.min) || !Array.isArray(b.max)) return null;
  const [x0, y0] = b.min as number[];
  const [x1, y1] = b.max as number[];
  return [x0, y0, x1, y1].every(Number.isFinite) ? { x0, y0, x1, y1 } : null;
}

export interface DesignUi {
  /** Edit and Tools dropdowns for the Design toolbar. */
  toolbar: ReactNode;
  /** Canvas context-menu entries. */
  menuItems: MenuItem[];
  /** Entries for the Arrange dropdown. */
  arrangeItems: MenuItem[];
  dialogs: ReactNode;
  /** The Art Library dock. */
  dock: ReactNode;
  /** Shape Properties for a lone primitive. */
  properties: ReactNode;
  wrap: (inner: CanvasTool) => CanvasTool;
  handlers: Record<string, KeyHandler>;
  modalOpen: boolean;
  /** App's Ctrl+C / X / V and Alt+V go here; true when taken. */
  clipKey: (kind: "copy" | "cut" | "paste" | "pasteInPlace", e: KeyboardEvent) => boolean;
  copy: (cut: boolean) => void;
  paste: (inPlace: boolean) => void;
  canPaste: boolean;
  openArray: () => void;
}

export function useDesignUi(p: DesignUiProps): DesignUi {
  const t = useT();
  const P = useRef(p);
  P.current = p;
  const [dlg, setDlg] = useState<Dlg | null>(null);
  const [sysClip, setSysClipRaw] = useState(loadSysClip);
  const [clipTick, setClipTick] = useState(0);
  const clip = useRef<Clip | null>(null);
  const sess = useMemo(() => store<Session>(null), []);
  const session = useSyncExternalStore(sess.sub, sess.get);
  const lastTap = useRef<{ t: number; index: number } | null>(null);
  const pending = useRef<{ kind: "copy" | "cut" | "paste"; timer: number } | null>(null);

  const setSysClip = (v: boolean) => {
    setSysClipRaw(v);
    try {
      localStorage.setItem(SYS_KEY, v ? "on" : "off");
    } catch {
      // Storage unavailable: the choice lasts for this visit.
    }
  };
  // The LightBurn preferences import sets "Use system clipboard" (UseSystemClipboard).
  useEffect(() => {
    const on = (e: Event) => {
      const v = (e as CustomEvent<unknown>).detail;
      if (typeof v === "boolean") setSysClip(v);
    };
    window.addEventListener(SYSTEM_CLIPBOARD_EVENT, on);
    return () => window.removeEventListener(SYSTEM_CLIPBOARD_EVENT, on);
  }, []);
  const notify: Notify = (m, k) => P.current.notify(m, k);
  const units = (): string[] => (P.current.doc ? unitsOf(P.current.doc, P.current.selection.objects) : []);
  const layer = (): string | null => P.current.activeLayer ?? P.current.doc?.layers[0]?.id ?? null;
  const selectResult = (r: unknown) => {
    const ids = G.resultIds(r);
    if (ids.length) P.current.selectIds(ids);
  };
  const exec = (inv: Invocation | Invocation[], after?: (r: unknown) => void) =>
    P.current
      .run(inv)
      .then((r) => after?.(r))
      .catch(P.current.report);
  /** Whether the engine has `cmd`; else says so (`missing`: the "This engine cannot ... yet." message). */
  const need = (cmd: string, missing: MsgKey) => {
    if (P.current.has(cmd)) return true;
    notify(tr(missing), "error");
    return false;
  };

  // ---- clipboard ------------------------------------------------------------------------------------------------

  const copyInternal = (cut: boolean): string | null => {
    const d = P.current.doc;
    const ids = P.current.selection.objects;
    if (!d || !ids.length) return null;
    const fragment = fragmentOf(d, ids);
    if (!fragment) return null;
    const marker = newMarker();
    clip.current = { fragment, marker, ids: fragment.objects.map((o) => o.id) };
    setClipTick((t) => t + 1);
    const n = fragment.objects.length;
    if (cut) void exec(C.remove(ids));
    notify(n === 1 ? tr(cut ? "dz.cut1" : "dz.copied1") : tr(cut ? "dz.cutN" : "dz.copiedN", { n }));
    return marker;
  };

  /** Write SVG to the system clipboard with the async API (the menu entries, and the real SVG type when allowed). */
  const writeSystem = async (svg: string, svgOnly = false) => {
    const nav = navigator as Navigator & { clipboard?: Clipboard };
    try {
      const CI = (window as unknown as { ClipboardItem?: typeof ClipboardItem }).ClipboardItem;
      const supportsSvg = !!CI && typeof (CI as unknown as { supports?: (t: string) => boolean }).supports === "function" && (CI as unknown as { supports: (t: string) => boolean }).supports("image/svg+xml");
      if (CI && nav.clipboard?.write && (supportsSvg || !svgOnly)) {
        const items: Record<string, Blob> = { "text/plain": new Blob([svg], { type: "text/plain" }) };
        if (supportsSvg) items["image/svg+xml"] = new Blob([svg], { type: "image/svg+xml" });
        await nav.clipboard.write([new CI(items)]);
      } else if (!svgOnly) await nav.clipboard?.writeText?.(svg);
    } catch {
      // Not allowed here (no permission or an insecure context): the internal copy still works.
    }
  };

  const copy = (cut: boolean, data?: DataTransfer | null) => {
    const d = P.current.doc;
    const ids = P.current.selection.objects;
    const marker = copyInternal(cut);
    if (!marker || !sysClip || !d) return;
    const text = selectionSvg(d, ids, marker);
    if (!text) return;
    if (data) {
      data.setData("text/plain", text);
      try {
        data.setData("image/svg+xml", text);
      } catch {
        // Some browsers only take standard types here.
      }
      void writeSystem(text, true);
    } else void writeSystem(text);
  };

  const at = (inPlace: boolean): Vec2 | null => (inPlace ? null : P.current.cursor.current);

  const pasteInternal = (inPlace: boolean) => {
    const c = clip.current;
    const d = P.current.doc;
    if (!c || !d) return;
    const pos = at(inPlace);
    if (P.current.has(DCMD.artInsert)) {
      const invs = pasteInvocations(c.fragment, pos);
      if (!invs.length) return;
      void exec(invs.length === 1 ? invs[0] : invs, selectResult);
      return;
    }
    // Older engines: duplicate the originals while they exist.
    if (c.ids.every((id) => d.objects.some((o) => o.id === id)) && P.current.has(C.OPT.duplicate)) {
      const b = fragmentBounds(c.fragment);
      const off: Vec2 = pos && b ? [Math.round((pos[0] - (b.x0 + b.x1) / 2) * 1e4) / 1e4, Math.round((pos[1] - (b.y0 + b.y1) / 2) * 1e4) / 1e4] : [0, 0];
      void exec(C.duplicate(c.ids, off), selectResult);
      return;
    }
    notify(tr("dz.copiedGone"), "error");
  };

  /** Import pasted content from another program, centred on the pointer (or where it lands, pasting in place). */
  const pasteForeign = async (c: ClipContent, inPlace: boolean, file?: Blob | null) => {
    const pp = P.current;
    const pos = at(inPlace);
    const move = async (r: unknown) => {
      const ids = G.resultIds(r);
      const b = resultBox(r);
      if (pos && b && ids.length) {
        const d: Vec2 = [Math.round((pos[0] - (b.x0 + b.x1) / 2) * 1e4) / 1e4, Math.round((pos[1] - (b.y0 + b.y1) / 2) * 1e4) / 1e4];
        if (d[0] || d[1]) await pp.run(C.transform(ids, { translate: d }));
      }
      if (ids.length) pp.selectIds(ids);
    };
    try {
      if (c.kind === "svg") {
        const r = await pp.run(C.importSvg(c.svg, tr("dz.pastedArt"), layer()));
        await move(r);
      } else if (c.kind === "image" && file) {
        if (file.type === "image/svg+xml") {
          const r = await pp.run(C.importSvg(await file.text(), tr("dz.pastedArt"), layer()));
          await move(r);
          return;
        }
        if (file.size > 50 * 1024 * 1024) return notify(tr("dz.pasteTooLarge"), "error");
        const b64 = bytesToBase64(new Uint8Array(await file.arrayBuffer()));
        const r = await pp.run(C.importImage(b64, file.type || "image/png", tr("dz.pastedImage"), layer()));
        await move(r);
      } else if (c.kind === "text") {
        if (!pp.has("shape.text")) return notify(tr("dz.noText"), "error");
        const r = await pp.run(shapeText(c.text, pos ?? [0, 0], loadTextDefaults(), layer()));
        const ids = G.resultIds(r);
        if (ids.length) pp.selectIds(ids);
      }
    } catch (e) {
      pp.report(e);
    }
  };

  /** Paste what the system clipboard holds: our own copy from the internal one, anything else imported. */
  const pasteContent = (c: ClipContent, inPlace: boolean, file?: Blob | null, rawText?: string | null) => {
    if (c.kind === "internal") {
      if (clip.current && c.marker === clip.current.marker) return pasteInternal(inPlace);
      // Copied in another tab or window of OpenEtch: its SVG is all we have.
      const svg = svgIn(rawText);
      if (svg) return void pasteForeign({ kind: "svg", svg }, inPlace);
    }
    if (c.kind === "none") {
      if (clip.current) pasteInternal(inPlace);
      return;
    }
    void pasteForeign(c, inPlace, file);
  };

  /** The menu's Paste: reads the system clipboard with the async API when allowed, else pastes the internal copy. */
  const paste = (inPlace: boolean) => {
    const nav = navigator as Navigator & { clipboard?: Clipboard };
    if (!sysClip || !nav.clipboard) return pasteInternal(inPlace);
    const read = async () => {
      let text: string | null = null;
      let svg: string | null = null;
      let html: string | null = null;
      const files: File[] = [];
      if (nav.clipboard?.read) {
        const items = await nav.clipboard.read();
        for (const it of items) {
          for (const type of it.types) {
            const blob = await it.getType(type);
            if (type === "text/plain") text = await blob.text();
            else if (type === "image/svg+xml") svg = await blob.text();
            else if (type === "text/html") html = await blob.text();
            else if (type.startsWith("image/")) files.push(new File([blob], `pasted.${type.split("/")[1]}`, { type }));
          }
        }
      } else if (nav.clipboard?.readText) text = await nav.clipboard.readText();
      const c = classifyClipboard({ text, svg, html, files });
      pasteContent(c, inPlace, c.kind === "image" ? (c.file as File) : null, svg ?? text);
    };
    read().catch(() => pasteInternal(inPlace));
  };

  const live = useRef({ copy, paste, pasteInternal, pasteContent, copyInternal });
  live.current = { copy, paste, pasteInternal, pasteContent, copyInternal };

  // The page's copy / cut / paste events: Ctrl+C / X / V (and the browser's Edit menu) on the canvas.
  useEffect(() => {
    const ok = (e: Event) => {
      const pp = P.current;
      return pp.design && !pp.blocked && !typingIn(e.target) && !typingIn(document.activeElement) && !document.querySelector(".modal-backdrop");
    };
    const onCopy = (e: ClipboardEvent, cut: boolean) => {
      if (!ok(e) || textSelected() || !P.current.selection.objects.length) return;
      if (pending.current) {
        window.clearTimeout(pending.current.timer);
        pending.current = null;
      }
      e.preventDefault();
      live.current.copy(cut, e.clipboardData);
    };
    const onPaste = (e: ClipboardEvent) => {
      if (!ok(e)) return;
      if (pending.current) {
        window.clearTimeout(pending.current.timer);
        pending.current = null;
      }
      const dt = e.clipboardData;
      if (!dt) return;
      e.preventDefault();
      if (!loadSysClip()) return live.current.pasteInternal(false);
      const files = Array.from(dt.files ?? []);
      const text = dt.getData("text/plain");
      let svg = "";
      try {
        svg = dt.getData("image/svg+xml");
      } catch {
        svg = "";
      }
      const html = dt.getData("text/html");
      const c = classifyClipboard({ text, svg, html, files });
      live.current.pasteContent(c, false, c.kind === "image" ? (c.file as File) : null, svg || text);
    };
    const c1 = (e: ClipboardEvent) => onCopy(e, false);
    const c2 = (e: ClipboardEvent) => onCopy(e, true);
    document.addEventListener("copy", c1);
    document.addEventListener("cut", c2);
    document.addEventListener("paste", onPaste);
    return () => {
      document.removeEventListener("copy", c1);
      document.removeEventListener("cut", c2);
      document.removeEventListener("paste", onPaste);
    };
  }, []);

  /**
   * App's clipboard keys. With the system clipboard on, Ctrl+C / X / V are left to the browser so its copy / paste
   * event carries the data (a short fallback runs the internal action when no event comes); Alt+V and the internal
   * clipboard act at once.
   */
  const clipKey = (kind: "copy" | "cut" | "paste" | "pasteInPlace", e: KeyboardEvent): boolean => {
    const pp = P.current;
    if (!pp.design) return false;
    if ((kind === "copy" || kind === "cut") && !pp.selection.objects.length) return false;
    if (kind === "pasteInPlace") {
      e.preventDefault();
      paste(true);
      return true;
    }
    if (!sysClip) {
      e.preventDefault();
      if (kind === "paste") pasteInternal(false);
      else copyInternal(kind === "cut");
      return true;
    }
    if (pending.current) window.clearTimeout(pending.current.timer);
    const timer = window.setTimeout(() => {
      if (pending.current?.timer !== timer) return;
      pending.current = null;
      if (kind === "paste") live.current.pasteInternal(false);
      else live.current.copy(kind === "cut");
    }, 80);
    pending.current = { kind, timer };
    return true;
  };

  // ---- selection queries ----------------------------------------------------------------------------------------

  const selectBy = (cmd: typeof DCMD.selectOpen | typeof DCMD.selectOpenOnFill | typeof DCMD.selectContained) => {
    if (!need(cmd, cmd === DCMD.selectContained ? "dz.cannot.selectContained" : "dz.cannot.selectOpen")) return;
    const ids = cmd === DCMD.selectContained ? P.current.selection.objects : null;
    if (cmd === DCMD.selectContained && !ids?.length) return notify(tr("dz.selectContainers"), "error");
    execute(selectQuery(cmd, ids))
      .then((r) => {
        const found = (r.result as { ids?: unknown } | null)?.ids;
        const list = Array.isArray(found) ? found.filter((x): x is string => typeof x === "string") : [];
        if (!list.length) return notify(tr(cmd === DCMD.selectContained ? "dz.noneInside" : "dz.noOpen"));
        P.current.selectIds(list);
        notify(list.length === 1 ? tr("dz.selected1") : tr("dz.selectedN", { n: list.length }));
      })
      .catch(P.current.report);
  };

  // ---- warp / deform --------------------------------------------------------------------------------------------

  const startWarp = (mode: WarpKind) => {
    const d = P.current.doc;
    const ids = P.current.selection.objects;
    if (!d || !ids.length) return notify(tr("dz.selectToWarp"), "error");
    if (!need(DCMD.warp, "dz.cannot.warp")) return;
    const objs = d.objects.filter((o) => ids.includes(o.id));
    if (objs.some((o) => o.geometry.kind === "image")) return notify(tr("dz.noWarpImages"), "error");
    const box = unionBoxes(objs.map(objectBounds));
    if (!box || box.x1 - box.x0 <= 1e-6 || box.y1 - box.y0 <= 1e-6) return notify(tr("dz.warpNeedsSize"), "error");
    sess.set({ kind: "warp", mode, ids, box, handles: identityHandles(box, mode), drag: null, hover: null });
  };

  const applyWarp = () => {
    const s = sess.get();
    if (s?.kind !== "warp") return;
    if (handlesUnchanged(s.handles, identityHandles(s.box, s.mode))) {
      sess.set(null);
      return;
    }
    if (s.mode === "perspective") {
      const prob = perspectiveProblem(s.handles);
      if (prob) return notify(prob, "error");
    }
    sess.set(null);
    void exec(warp(warpParams(s.ids, s.mode, s.handles)), selectResult);
  };

  // ---- tangent circle -------------------------------------------------------------------------------------------

  const startTangent = () => {
    const d = P.current.doc;
    if (!d) return;
    if (!need(DCMD.tangentCandidates, "dz.cannot.tangent")) return;
    const ids = units();
    const circles = ids.filter((id) => {
      const o = d.objects.find((x) => x.id === id);
      return !!o && !!circleOf(o);
    });
    if (circles.length !== 3 || ids.length !== 3) return notify(tr("dz.threeCircles"), "error");
    execute(tangentCandidates(circles))
      .then((r) => {
        const cands = parseCandidates(r.result);
        if (!cands.length) return notify(tr("dz.noTangent"), "error");
        sess.set({ kind: "tangent", ids: circles, cands, hover: null });
      })
      .catch(P.current.report);
  };
  const createTangent = (pick: number[] | "all") => {
    const s = sess.get();
    if (s?.kind !== "tangent") return;
    sess.set(null);
    void exec(tangentCircle(s.ids, pick, P.current.activeLayer), selectResult);
  };

  // Enter / Esc while a session runs; a session whose shapes went away ends.
  useEffect(() => {
    if (!session) return;
    const on = (e: KeyboardEvent) => {
      if (typingIn(e.target) || document.querySelector(".modal-backdrop")) return;
      const s = sess.get();
      if (!s) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        sess.set(null);
      } else if (e.key === "Enter" && s.kind === "warp") {
        e.preventDefault();
        e.stopImmediatePropagation();
        applyWarp();
      }
    };
    window.addEventListener("keydown", on, true);
    return () => window.removeEventListener("keydown", on, true);
  });
  const doc = p.doc;
  useEffect(() => {
    const s = sess.get();
    if (!s) return;
    if (!doc || !p.design || s.ids.some((id) => !doc.objects.some((o) => o.id === id))) sess.set(null);
  }, [doc, p.design, sess]);

  // ---- canvas -----------------------------------------------------------------------------------------------------

  // The warp handles are hit within half the pointer's handle size (pointer.ts: 8 px for a mouse, 22 for a finger).
  const handleAt = (s: WarpSession, f: Vec2, scale: number, cls: PointerClass = "mouse"): number | null => {
    const tol = handleHitPx(cls) / 2 / Math.max(scale, 1e-6);
    let best: { i: number; d: number } | null = null;
    s.handles.forEach((h, i) => {
      const d = Math.hypot(h[0] - f[0], h[1] - f[1]);
      if (d <= tol && (!best || d < best.d)) best = { i, d };
    });
    return (best as { i: number } | null)?.i ?? null;
  };

  const owner = useRef<"design" | "inner" | null>(null);
  const dropCtx = useRef<RenderCtx | null>(null);

  const wrap = (inner: CanvasTool): CanvasTool => ({
    ...inner,
    down: (f, m, info, ctx) => {
      const s = sess.get();
      if (!s) {
        owner.current = "inner";
        return inner.down(f, m, info, ctx);
      }
      owner.current = "design";
      if (s.kind === "tangent") {
        const c = nearestCandidate(s.cands, f, 10 / ctx.scale);
        if (c) createTangent([c.index]);
        else notify(tr("dz.clickCircle"), "error");
        return true;
      }
      const i = handleAt(s, f, ctx.scale, pointerClass(info));
      if (i === null) return true;
      const lt = lastTap.current;
      lastTap.current = { t: info.time, index: i };
      if (lt && lt.index === i && info.time - lt.t < DBL_MS) {
        // Double-click resets the handle (LightBurn).
        const id = identityHandles(s.box, s.mode);
        const h = s.handles.slice();
        h[i] = id[i];
        sess.set({ ...s, handles: h, drag: null });
        return true;
      }
      sess.set({ ...s, drag: { index: i, base: s.handles.map((q) => [q[0], q[1]] as Vec2) } });
      return true;
    },
    drag: (f, m, ctx) => {
      if (owner.current !== "design") return inner.drag(f, m, ctx);
      const s = sess.get();
      if (s?.kind !== "warp" || !s.drag) return;
      let to = f;
      if (!m.shift && !m.alt) {
        const sp = ctx.snap(f, m, { exclude: s.ids });
        ctx.showSnap(sp.hit);
        to = sp.p;
      } else ctx.showSnap(null);
      sess.set({ ...s, handles: moveHandle(s.handles, s.drag.base, s.mode, s.drag.index, to, { h: m.shift, v: !!m.alt }) });
      ctx.readout(tr("dz.handleReadout", { n: s.drag.index + 1, x: to[0].toFixed(2), y: to[1].toFixed(2) }));
    },
    up: (f, m, ctx) => {
      if (owner.current !== "design") {
        owner.current = null;
        return inner.up(f, m, ctx);
      }
      owner.current = null;
      const s = sess.get();
      ctx.showSnap(null);
      ctx.readout(null);
      if (s?.kind === "warp" && s.drag) sess.set({ ...s, drag: null });
    },
    cancel: () => {
      if (owner.current !== "design") inner.cancel();
      owner.current = null;
      const s = sess.get();
      if (s?.kind === "warp" && s.drag) sess.set({ ...s, handles: s.drag.base, drag: null });
    },
    hover: (f: Vec2, m, ctx: CanvasCtx) => {
      const s = sess.get();
      if (!s) return inner.hover(f, m, ctx);
      if (s.kind === "warp") {
        const i = handleAt(s, f, ctx.scale);
        if (i !== s.hover) sess.set({ ...s, hover: i });
      } else {
        const c = nearestCandidate(s.cands, f, 10 / ctx.scale);
        const i = c ? c.index : null;
        if (i !== s.hover) sess.set({ ...s, hover: i });
      }
      ctx.showSnap(null);
      return true;
    },
    render: (r) => (
      <>
        {inner.render(r)}
        <SessionLayer sess={sess} doc={P.current.doc} r={r} />
      </>
    ),
    overlay: (r) => {
      dropCtx.current = r;
      return (
        <>
          {inner.overlay(r)}
          <HudItem id="design-session" slot="tl" rank={22} minW={200}>
            <SessionBar
              sess={sess}
              onApply={applyWarp}
              onCancel={() => sess.set(null)}
              onReset={() => {
                const s = sess.get();
                if (s?.kind === "warp") sess.set({ ...s, handles: identityHandles(s.box, s.mode) });
              }}
              onMode={(mode) => {
                const s = sess.get();
                if (s?.kind === "warp") sess.set({ ...s, mode, handles: mode === "grid" || s.mode === "grid" ? identityHandles(s.box, mode) : s.handles });
              }}
              onCreateAll={() => createTangent("all")}
            />
          </HudItem>
          <ArtDropTarget
            ctx={dropCtx}
            onDrop={(library, id, f) => insertArt(library, id, f)}
          />
        </>
      );
    },
  });

  const insertArt = (library: string, id: string, center: Vec2 | null) => {
    artApi
      .insert(library, id, center, null)
      .then((r) => {
        // The answer comes from the art route, not /api/execute: refetch like an edit.
        P.current.refreshDoc();
        selectResult(r.result);
      })
      .catch(P.current.report);
  };

  // ---- menus ------------------------------------------------------------------------------------------------------

  const has = p.has;
  const nSel = p.selection.objects.length;
  const nUnits = p.doc ? unitsOf(p.doc, p.selection.objects).length : 0;
  // LightBurn's virtual array context menu (Edit / Duplicate / Flatten / Remove from / Add to Array).
  const varr = virtualArrayOf(p.doc, p.selection.objects);
  const inArray = new Set((p.doc?.virtual_arrays ?? []).flatMap((a) => [...a.sources, ...a.clones]));
  const freeSel = p.selection.objects.filter((id) => !inArray.has(id));
  const varrOriginals = varr ? p.selection.objects.filter((id) => varr.sources.includes(id)) : [];
  const arrayItems: MenuItem[] =
    varr && has("array.flatten")
      ? [
          { sep: true },
          { label: t("dz.va.edit"), action: () => setDlg({ kind: "array", edit: varr }) },
          { label: t("dz.va.duplicate"), action: () => void exec({ id: "array.duplicate", params: { array: varr.id } }, (r) => (selectResult(r), notify(tr("dz.va.duplicated")))) },
          { label: t("dz.va.flatten"), action: () => void exec({ id: "array.flatten", params: { arrays: [varr.id] } }, () => notify(tr("dz.va.flattened"))) },
          { label: t("dz.va.removeFrom"), disabled: !varrOriginals.length, action: () => void exec({ id: "array.remove", params: { ids: varrOriginals } }) },
          { label: t("dz.va.addTo"), disabled: !freeSel.length, action: () => void exec({ id: "array.add", params: { array: varr.id, ids: freeSel } }) },
        ]
      : [];
  void clipTick;
  const canPaste = !!clip.current || sysClip;
  // Edit ▾ (docs/ux/ui-plan-2026-11.md P2): the clipboard and selection commands in one place. Undo, Redo, Duplicate,
  // Delete, Select all and Invert run their hotkeys.ts actions (the same handlers as their keys).
  const run = (id: string) => () => void runHotkeyAction(id);
  const editItems: MenuItem[] = [
    { label: t("rb.undo"), icon: "undo", actionId: "edit.undo", action: run("edit.undo") },
    { label: t("rb.redo"), icon: "redo", actionId: "edit.redo", action: run("edit.redo") },
    { sep: true },
    { label: t("dz.menu.cut"), hint: "Ctrl+X", disabled: !nSel, action: () => copy(true) },
    { label: t("dz.menu.copy"), hint: "Ctrl+C", disabled: !nSel, action: () => copy(false) },
    { label: t("dz.menu.paste"), hint: "Ctrl+V", disabled: !canPaste, action: () => paste(false) },
    { label: t("dz.menu.pasteInPlace"), hint: "Alt+V", disabled: !canPaste, action: () => paste(true) },
    { label: t("rb.duplicate"), actionId: "edit.duplicate", disabled: !nSel, action: run("edit.duplicate") },
    { label: t("rb.delete"), icon: "delete", actionId: "edit.delete", disabled: !nSel, danger: true, action: run("edit.delete") },
    { sep: true },
    { label: t("rb.selectAll"), actionId: "edit.selectAll", disabled: !p.doc?.objects.length, action: run("edit.selectAll") },
    { label: t("rb.invertSelection"), actionId: "edit.invert", disabled: !p.doc?.objects.length, action: run("edit.invert") },
    {
      label: t("rb.selectBy"),
      submenu: [
        { label: t("dz.menu.selectOpen"), actionId: "edit.selectOpen", disabled: !has(DCMD.selectOpen), action: () => selectBy(DCMD.selectOpen) },
        { label: t("dz.menu.selectOpenFill"), disabled: !has(DCMD.selectOpenOnFill), action: () => selectBy(DCMD.selectOpenOnFill) },
        { label: t("dz.menu.selectContained"), disabled: !nSel || !has(DCMD.selectContained), action: () => selectBy(DCMD.selectContained) },
      ],
    },
    { sep: true },
    { label: t("rb.sysClip"), checked: sysClip, action: () => setSysClip(!sysClip) },
    { label: t("dz.menu.curveTolerance"), disabled: !p.doc || !has(DCMD.curveTolerance), action: () => setDlg({ kind: "curveTolerance" }) },
  ];
  const toolsItems: MenuItem[] = [
    { label: t("dz.menu.warp"), actionId: "tools.warp", disabled: !nSel || !has(DCMD.warp), action: () => startWarp("perspective") },
    { label: t("dz.menu.deform"), actionId: "tools.deform", disabled: !nSel || !has(DCMD.warp), action: () => startWarp("grid") },
    { sep: true },
    { label: t("dz.menu.tangent"), actionId: "tools.tangentCircle", disabled: nUnits !== 3 || !has(DCMD.tangentCandidates), action: startTangent },
    {
      label: t("dz.menu.rubberBand"),
      disabled: !nSel || !has(DCMD.rubberBand),
      action: () => need(DCMD.rubberBand, "dz.cannot.rubberBand") && void exec(rubberBand(units(), p.activeLayer), selectResult),
    },
    {
      label: nSel ? t("dz.menu.toolpathSel") : t("dz.menu.toolpath"),
      disabled: !p.doc?.objects.length || !has(DCMD.toolpathToPath),
      action: () => need(DCMD.toolpathToPath, "dz.cannot.toolpath") && void exec(toolpathToPath(nSel ? units() : null), selectResult),
    },
    { sep: true },
    { label: t("dz.menu.artLibrary"), actionId: "window.artLibrary", action: openArtDock },
    ...(p.extraTools?.length ? [{ sep: true } as MenuItem, ...p.extraTools] : []),
  ];
  const arrangeItems: MenuItem[] = [
    { sep: true },
    { label: t("dz.menu.movePosition"), actionId: "arrange.movePosition", disabled: !nSel || !has(DCMD.pagePosition), action: () => setDlg({ kind: "movePosition" }) },
    { label: t("dz.menu.moveH"), actionId: "arrange.moveHTogether", disabled: nUnits < 2, action: () => setDlg({ kind: "moveTogether", axis: "x" }) },
    { label: t("dz.menu.moveV"), actionId: "arrange.moveVTogether", disabled: nUnits < 2, action: () => setDlg({ kind: "moveTogether", axis: "y" }) },
    ...arrayItems,
  ];
  const menuItems: MenuItem[] = [
    { label: t("dz.menu.pasteInPlace"), hint: "Alt+V", section: "top", disabled: !canPaste, action: () => paste(true) },
    { label: t("dz.menu.warp"), section: "modify", disabled: !nSel || !has(DCMD.warp), action: () => startWarp("perspective") },
    { label: t("dz.menu.deform"), section: "modify", disabled: !nSel || !has(DCMD.warp), action: () => startWarp("grid") },
    // The Art library is a dock: a desktop feature (the phone shows no docks).
    { label: t("dz.menu.addArt"), section: "modify", desktopOnly: true, disabled: !nSel, action: openArtDock },
    ...arrayItems.map((it) => (it.sep ? it : { ...it, section: "arrange" })),
  ];

  // The phone leaves out the desktop features (workspace/desktopOnly.ts): the Art library and main's Add Tabs tool.
  const layout = useLayoutMode().layout;
  const toolsShown = forLayout(toolsItems, layout);
  usePublish("menu.edit", { label: t("dz.edit"), items: editItems });
  usePublish("menu.tools", { label: t("dz.tools"), items: toolsShown });
  const toolbar = p.design ? (
    <>
      <RibbonSlot group="modify" order={30}>
        <Dropdown icon="edit" label={t("dz.edit")} title={t("dz.editTitle")} items={editItems} />
        <Dropdown icon="tools" label={t("dz.tools")} title={t("dz.toolsTitle")} items={toolsShown} />
      </RibbonSlot>
      {desktopFeatures(layout) && (
        <RibbonSlot group="insert" order={10}>
          <button type="button" className="tool-btn" onClick={openArtDock} title={t("rb.artTitle")} aria-label={t("rb.art")}>
            <Icon name="art" size={20} />
            <span className="tool-caption">{t("rb.art")}</span>
          </button>
        </RibbonSlot>
      )}
    </>
  ) : null;

  const close = useCallback(() => setDlg(null), []);
  const ids = units();
  const dialogs =
    dlg && p.doc ? (
      dlg.kind === "movePosition" ? (
        <MovePositionDialog doc={p.doc} ids={ids} run={p.run} onClose={close} />
      ) : dlg.kind === "moveTogether" ? (
        <MoveTogetherDialog ids={ids} axis={dlg.axis} run={p.run} onClose={close} />
      ) : dlg.kind === "curveTolerance" ? (
        <CurveToleranceDialog doc={p.doc} run={p.run} onClose={close} />
      ) : (
        <GridArrayDialog
          doc={p.doc}
          ids={p.selection.objects}
          workspaceMm={p.workspaceMm}
          has={p.has}
          run={p.run}
          onClose={close}
          editArray={dlg.edit ?? null}
          onCreated={(created) => P.current.selectIds([...P.current.selection.objects, ...created])}
        />
      )
    ) : null;

  const dock = <ArtLibraryDock selection={p.selection.objects} design={p.design} notify={p.notify} onInsert={(lib, id, c) => insertArt(lib, id, c)} />;
  const properties = p.design ? <ShapeProperties doc={p.doc} selection={p.selection} has={p.has} run={p.run} report={p.report} /> : null;

  const handlers: Record<string, KeyHandler> = {
    "tools.warp": () => (p.design ? startWarp("perspective") : false),
    "tools.deform": () => (p.design ? startWarp("grid") : false),
    "tools.tangentCircle": () => (p.design ? startTangent() : false),
    "edit.selectOpen": () => (p.design ? selectBy(DCMD.selectOpen) : false),
    "window.artLibrary": () => openArtDock(),
    "arrange.movePosition": () => (p.design && nSel ? setDlg({ kind: "movePosition" }) : false),
  };

  return {
    toolbar,
    menuItems,
    arrangeItems,
    dialogs,
    dock,
    properties,
    wrap,
    handlers,
    modalOpen: dlg !== null,
    clipKey,
    copy: (cut) => copy(cut),
    paste,
    canPaste,
    openArray: () => setDlg({ kind: "array" }),
  };
}

// ---- drawing ------------------------------------------------------------------------------------------------------

function SessionLayer(p: { sess: ReturnType<typeof store<Session>>; doc: GDocument | null; r: RenderCtx }) {
  const s = useSyncExternalStore(p.sess.sub, p.sess.get);
  const { r } = p;
  const preview = useMemo(() => {
    if (s?.kind !== "warp" || !p.doc) return null;
    const objs = p.doc.objects.filter((o) => s.ids.includes(o.id));
    return warpPreview(objs, s.box, s.mode, s.handles);
  }, [s, p.doc]);
  if (!s) return null;
  const px = r.px;
  if (s.kind === "tangent") {
    return (
      <g className="dz-layer" pointerEvents="none">
        {s.cands.map((c) => (
          <circle key={c.index} className={`dz-tangent${s.hover === c.index ? " hot" : ""}`} cx={c.center[0]} cy={c.center[1]} r={c.radius_mm} strokeWidth={px(s.hover === c.index ? 2.5 : 1.4)} />
        ))}
      </g>
    );
  }
  const h = s.handles;
  const mesh: string[] = [];
  if (s.mode === "grid") {
    for (let row = 0; row < 4; row++) mesh.push(`M${h.slice(row * 4, row * 4 + 4).map((q) => `${q[0]},${q[1]}`).join("L")}`);
    for (let col = 0; col < 4; col++) mesh.push(`M${[0, 1, 2, 3].map((row) => `${h[row * 4 + col][0]},${h[row * 4 + col][1]}`).join("L")}`);
  } else mesh.push(`M${h.map((q) => `${q[0]},${q[1]}`).join("L")}Z`);
  const b = s.box;
  const hs = px(4.5);
  return (
    <g className="dz-layer" pointerEvents="none">
      <rect className="dz-warp-box" x={b.x0} y={b.y0} width={b.x1 - b.x0} height={b.y1 - b.y0} strokeWidth={px(1)} strokeDasharray={`${px(4)} ${px(3)}`} />
      {preview === null ? null : <path className="dz-warp-preview" d={preview} strokeWidth={px(1.4)} />}
      <path className="dz-warp-mesh" d={mesh.join("")} strokeWidth={px(1)} />
      {h.map((q, i) => (
        <rect
          key={i}
          className={`dz-warp-handle${s.hover === i || s.drag?.index === i ? " hot" : ""}`}
          x={q[0] - hs}
          y={q[1] - hs}
          width={2 * hs}
          height={2 * hs}
          strokeWidth={px(1.2)}
        />
      ))}
    </g>
  );
}

function SessionBar(p: {
  sess: ReturnType<typeof store<Session>>;
  onApply: () => void;
  onCancel: () => void;
  onReset: () => void;
  onMode: (m: WarpKind) => void;
  onCreateAll: () => void;
}) {
  const t = useT();
  const s = useSyncExternalStore(p.sess.sub, p.sess.get);
  if (!s) return null;
  if (s.kind === "tangent")
    return (
      <div className="vp-overlay et-bar" role="toolbar" aria-label={t("dz.tangentCircle")}>
        <span>
          <b>{t("dz.tangentCircle")}</b>: {t("dz.tangentBar", { n: s.cands.length })}
        </span>
        <button className="btn btn-small" onClick={p.onCreateAll}>
          {t("dz.createAll")}
        </button>
        <button className="btn btn-small" onClick={p.onCancel}>
          {t("dz.cancel")}
        </button>
      </div>
    );
  const bad = s.mode === "perspective" ? perspectiveProblem(s.handles) : null;
  return (
    <div className="vp-overlay et-bar" role="toolbar" aria-label={s.mode === "grid" ? t("dz.deform") : t("dz.warp")}>
      <b>{s.mode === "grid" ? t("dz.deform") : t("dz.warp")}</b>
      {s.mode !== "grid" && (
        <label className="et-opt">
          <select value={s.mode} onChange={(e) => p.onMode(e.target.value as WarpKind)} aria-label={t("dz.warpKind")}>
            <option value="perspective">{t("dz.perspective")}</option>
            <option value="envelope">{t("dz.envelope")}</option>
          </select>
        </label>
      )}
      <span className="et-hint">
        {t("dz.warpHint", { n: s.handles.length })}
      </span>
      {bad && <span className="form-error">{bad}</span>}
      <button className="btn btn-small" onClick={p.onReset}>
        {t("dz.reset")}
      </button>
      <button className="btn btn-small" onClick={p.onCancel}>
        {t("dz.cancel")}
      </button>
      <button className="btn btn-small btn-primary" onClick={p.onApply} disabled={!!bad}>
        {t("dz.apply")}
      </button>
    </div>
  );
}

/** Takes Art Library items dropped on the canvas; the item lands centred on the drop point. */
function ArtDropTarget(p: { ctx: { current: RenderCtx | null }; onDrop: (library: string, id: string, f: Vec2) => void }) {
  const ref = useRef<HTMLSpanElement>(null);
  const onDrop = useRef(p.onDrop);
  onDrop.current = p.onDrop;
  const ctx = p.ctx;
  useEffect(() => {
    const vp = ref.current?.closest(".viewport") as HTMLElement | null;
    if (!vp) return;
    const ours = (e: DragEvent) => !!e.dataTransfer && Array.from(e.dataTransfer.types).includes(ART_DRAG_TYPE);
    const over = (e: DragEvent) => {
      if (!ours(e)) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };
    const drop = (e: DragEvent) => {
      if (!ours(e)) return;
      e.preventDefault();
      const r = ctx.current;
      const svg = vp.querySelector("svg.vp-main") ?? vp;
      const rect = svg.getBoundingClientRect();
      let item: { library?: unknown; id?: unknown } = {};
      try {
        item = JSON.parse(e.dataTransfer?.getData(ART_DRAG_TYPE) ?? "{}") as typeof item;
      } catch {
        return;
      }
      if (!r || typeof item.library !== "string" || typeof item.id !== "string") return;
      const f = r.toField([e.clientX - rect.left, e.clientY - rect.top]);
      onDrop.current(item.library, item.id, [Math.round(f[0] * 1e4) / 1e4, Math.round(f[1] * 1e4) / 1e4]);
    };
    vp.addEventListener("dragover", over);
    vp.addEventListener("drop", drop);
    return () => {
      vp.removeEventListener("dragover", over);
      vp.removeEventListener("drop", drop);
    };
  }, [ctx]);
  return <span ref={ref} hidden />;
}
