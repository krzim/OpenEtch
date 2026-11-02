// Text and barcodes on the canvas, LightBurn-style:
// - Text tool (T; LightBurn's Ctrl+T where the browser lets a page have it): click to place a text box at the click
//   point (snapped) and type; Enter starts a new line; Esc, Ctrl+Enter or a click elsewhere finishes (an empty box adds
//   nothing). Clicking an existing text with the tool, or double-clicking one with the Select tool, edits it in place;
//   emptying it deletes it. While the tool is active an options bar (LightBurn's Text Options toolbar) sets the font,
//   height, spacing, alignment and styles of new text, and of the selected text when one is selected.
// - Barcode tool: click to place; the Create Bar Code dialog sets the kind and data. Double-click a barcode to edit it.
// - Properties: the Text section (all Text Options and Shape Properties of a text: font picker, spacing, alignment,
//   styles, bend, max width / squeeze, text on a path, variable text with LightBurn's code syntax, ignore empty vars,
//   convert to path), Bar Code Properties, Apply Path to Text for a text + shape selection, and the Variable Text
//   window (docked) while the document uses variable text or it was opened.
// - A badge on every text whose font the engine does not have.
// The engine is the source of truth: everything is a command (text/textModel.ts builds them).

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { execute, type Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { createdIds } from "../api/drop.ts";
import type { GDocument, Vec2 } from "../api/types.ts";
import type { CanvasCtx, CanvasTool, PointerInfo, RenderCtx } from "../canvasTool.ts";
import type { MenuItem } from "../components/ContextMenu.tsx";
import type { Tool } from "../components/Viewport.tsx";
import { Icon } from "../components/ui.tsx";
import { RibbonSlot } from "../components/Ribbon.tsx";
import { openCommandSearch } from "../components/commandBus.ts";
import type { Mods } from "../editing.ts";
import { objectBounds } from "../geometry.ts";
import { unitsOf, type Selection } from "../selection.ts";
import { useT, useUnitConversion } from "../workspace/usePrefs.ts";
import { trimNumber } from "../workspace/units.ts";
import { ContextStripPortal } from "../workspace/contextStrip.tsx";
import { HudItem } from "../hud/CanvasHud.tsx";
import { BarcodeDialog, NEW_BARCODE } from "./Barcode.tsx";
import { FontPicker } from "./FontPicker.tsx";
import { ApplyPathCard, BarcodeSection, Segmented, TextSection, alignOptions, warnFrom } from "./TextProperties.tsx";
import {
  TXT,
  applyPath,
  applyPathPair,
  barcodeCreate,
  barcodeParams,
  barcodeRedraw,
  cssFontFamily,
  cssTextAlign,
  defaultsFrom,
  defaultsToFields,
  detachPath,
  editorTranslate,
  fontList,
  fontMissing,
  hasInk,
  isText,
  loadTextDefaults,
  normalizeText,
  originOf,
  parseFontList,
  rotationOf,
  saveTextDefaults,
  scaleOf,
  shapeText,
  textUpdate,
  usesVariables,
  type BarcodeParams,
  type FontInfo,
  type TextDefaults,
} from "./textModel.ts";
import { VarTextWindow } from "./VarTextWindow.tsx";
import "./text.css";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export interface TextToolsProps {
  doc: GDocument | null;
  selection: Selection;
  selectIds: (ids: string[]) => void;
  tool: Tool;
  setTool: (t: Tool) => void;
  activeLayer: string | null;
  /** The Design workspace is shown. */
  design: boolean;
  has: (cmd: string) => boolean;
  run: Run;
  notify: (msg: string, kind?: "info" | "error") => void;
  report: (e: unknown) => void;
}

export interface TextTools {
  /** The canvas tool to hand the Viewport: text first, then `inner` (EditTools) for everything else. */
  wrap: (inner: CanvasTool) => CanvasTool;
  toolbar: ReactNode;
  properties: ReactNode;
  dialogs: ReactNode;
  menuItems: MenuItem[];
  /** Hotkey handlers (workspace/hotkeys.ts ids). */
  handlers: Record<string, () => boolean | void>;
}

/** The on-canvas text editor. */
interface Editor {
  /** The text being edited, or null for a new one. */
  id: string | null;
  /** Field point of the alignment point. */
  at: Vec2;
  value: string;
  rot: number;
  scale: number;
}

type BarcodeDlg = { mode: "create"; at: Vec2 } | { mode: "edit"; id: string };

const DOUBLE_TAP_MS = 450;
const DOUBLE_TAP_PX = 6;

export function useTextTools(p: TextToolsProps): TextTools {
  const t = useT();
  const P = useRef(p);
  P.current = p;
  const { doc, selection, tool, design } = p;
  const canText = p.has(TXT.shapeText);
  const canBarcode = p.has(TXT.barcode);

  // ---- fonts ----
  const [fonts, setFonts] = useState<FontInfo[] | null>(null);
  const canFonts = p.has(TXT.fontList);
  const loadFonts = useCallback(() => {
    execute(fontList())
      .then((r) => setFonts(parseFontList(r.result)))
      .catch(() => setFonts(null));
  }, []);
  useEffect(() => {
    if (canFonts) loadFonts();
  }, [canFonts, loadFonts]);

  // ---- defaults for new text (this browser) ----
  const [defaults, setDefaultsRaw] = useState<TextDefaults>(loadTextDefaults);
  const setDefaults = (d: TextDefaults) => {
    setDefaultsRaw(d);
    saveTextDefaults(d);
  };

  // ---- state ----
  const [editor, setEditorRaw] = useState<Editor | null>(null);
  const editorRef = useRef<Editor | null>(null);
  const setEditor = (e: Editor | null) => {
    editorRef.current = e;
    setEditorRaw(e);
  };
  const areaRef = useRef<HTMLTextAreaElement | null>(null);
  const [pickPath, setPickPath] = useState<string | null>(null);
  const [barcodeDlg, setBarcodeDlg] = useState<BarcodeDlg | null>(null);
  const [lastBarcode, setLastBarcode] = useState<BarcodeParams>(NEW_BARCODE);
  const [varOpen, setVarOpen] = useState(false);
  const lastTap = useRef<{ time: number; screen: Vec2; id: string } | null>(null);
  const owner = useRef<"text" | "inner" | null>(null);

  const report = (e: unknown) => P.current.report(e);
  const objOf = (id: string | null | undefined) => (id ? (P.current.doc?.objects.find((o) => o.id === id) ?? null) : null);

  /** Finish the on-canvas editor: add, change or (emptied) delete the text. */
  const commitEditor = () => {
    const ed = editorRef.current;
    if (!ed) return;
    setEditor(null);
    const text = normalizeText(ed.value);
    const pp = P.current;
    if (ed.id) {
      const o = objOf(ed.id);
      if (!isText(o)) return;
      if (!hasInk(text)) void pp.run(C.remove([ed.id])).catch(report);
      else if (text !== o.geometry.text)
        void pp
          .run(textUpdate(ed.id, { text }))
          .then((r) => warnFrom(r, pp.notify))
          .catch(report);
      return;
    }
    if (!hasInk(text)) return;
    void pp
      .run(shapeText(text, ed.at, defaultsRef.current, pp.activeLayer))
      .then((r) => {
        warnFrom(r, pp.notify);
        const ids = createdIds(r);
        if (ids.length) pp.selectIds(ids);
      })
      .catch(report);
  };
  const defaultsRef = useRef(defaults);
  defaultsRef.current = defaults;

  /** Open the editor on text `id`. */
  const editText = (id: string) => {
    const o = objOf(id);
    if (!isText(o)) return;
    commitEditor();
    if (o.geometry.path) {
      // On a path the alignment point is the path's frame: edit over the text's bounds instead.
      const b = objectBounds(o);
      const at: Vec2 = b ? [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2] : originOf(o);
      setEditor({ id, at, value: o.geometry.text, rot: 0, scale: scaleOf(o) });
    } else setEditor({ id, at: originOf(o), value: o.geometry.text, rot: rotationOf(o), scale: scaleOf(o) });
    P.current.selectIds([id]);
  };

  const editBarcode = (id: string) => {
    if (barcodeParams(objOf(id))) setBarcodeDlg({ mode: "edit", id });
  };

  // Leaving the Design workspace or the text tool (other than to Select, which edits by double-click) finishes editing.
  useEffect(() => {
    if (!design || (tool !== "text" && tool !== "select")) commitEditor();
    if (tool !== "select" && tool !== "text") setPickPath(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tool, design]);
  // The edited text was deleted (undo, an agent): drop the editor.
  useEffect(() => {
    const ed = editorRef.current;
    if (ed?.id && !doc?.objects.some((o) => o.id === ed.id)) setEditor(null);
    if (pickPath && !doc?.objects.some((o) => o.id === pickPath)) setPickPath(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc]);

  // Esc cancels picking a path for Apply Path to Text.
  useEffect(() => {
    if (!pickPath) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      setPickPath(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pickPath]);

  // ---- the selection ----
  const sel = selection.objects;
  const single = sel.length === 1 && doc ? (doc.objects.find((o) => o.id === sel[0]) ?? null) : null;
  const singleText = isText(single) ? single : null;
  const singleBarcode = single && barcodeParams(single) ? single : null;
  const pair = applyPathPair(doc, sel);
  const multi = doc ? unitsOf(doc, sel).length >= 2 : false;

  /** What the options bar shows and edits: the text being edited or the selected text, else the defaults. */
  const barTarget = editor ? objOf(editor.id) : singleText;
  const barTextObj = isText(barTarget) ? barTarget : null;
  const barValues: TextDefaults = barTextObj ? defaultsFrom(barTextObj.geometry) : defaults;
  const barChange = (c: Partial<TextDefaults>) => {
    setDefaults({ ...barValues, ...c, font: c.font !== undefined ? c.font : barValues.font });
    if (barTextObj)
      void P.current
        .run(textUpdate(barTextObj.id, defaultsToFields(c)))
        .then((r) => warnFrom(r, P.current.notify))
        .catch(report);
    requestAnimationFrame(() => areaRef.current?.focus());
  };

  // ---- canvas tool ----
  const down = (f: Vec2, m: Mods, info: PointerInfo, ctx: CanvasCtx): boolean => {
    const pp = P.current;
    if (!pp.design) return false;
    if (pickPath) {
      const hit = ctx.hitObject(f);
      const o = objOf(hit);
      if (!o || o.id === pickPath) {
        pp.notify(t("text.pickPathMiss"));
        return true;
      }
      if (o.geometry.kind === "image" || o.geometry.kind === "text") {
        pp.notify(t("text.pickPathNotVector"), "error");
        return true;
      }
      const textId = pickPath;
      setPickPath(null);
      void pp.run(applyPath(textId, o.id)).catch(report);
      pp.selectIds([textId]);
      return true;
    }
    if (editorRef.current) {
      commitEditor();
      if (pp.tool !== "text") return true;
    }
    if (pp.tool === "text") {
      if (!canText) {
        pp.notify(t("text.unsupported"), "error");
        return true;
      }
      const hit = objOf(ctx.hitObject(f));
      if (isText(hit)) {
        editText(hit.id);
        return true;
      }
      const at = ctx.snap(f, m).p;
      ctx.showSnap(null);
      if (pp.selection.objects.length) pp.selectIds([]);
      setEditor({ id: null, at, value: "", rot: 0, scale: 1 });
      return true;
    }
    if (pp.tool === "barcode") {
      if (!canBarcode) {
        pp.notify(t("barcode.unsupported"), "error");
        return true;
      }
      const hit = objOf(ctx.hitObject(f));
      if (hit && barcodeParams(hit)) {
        pp.selectIds([hit.id]);
        editBarcode(hit.id);
        return true;
      }
      setBarcodeDlg({ mode: "create", at: ctx.snap(f, m).p });
      ctx.showSnap(null);
      return true;
    }
    if (pp.tool === "select") {
      // Double-click a text to edit it on the canvas, a barcode to open its dialog (LightBurn).
      const id = ctx.hitObject(f);
      const prev = lastTap.current;
      lastTap.current = id ? { time: info.time, screen: info.screen, id } : null;
      if (
        id &&
        prev &&
        prev.id === id &&
        info.time - prev.time < DOUBLE_TAP_MS &&
        Math.hypot(info.screen[0] - prev.screen[0], info.screen[1] - prev.screen[1]) < DOUBLE_TAP_PX
      ) {
        const o = objOf(id);
        if (isText(o)) {
          lastTap.current = null;
          editText(id);
          return true;
        }
        if (o && barcodeParams(o)) {
          lastTap.current = null;
          editBarcode(id);
          return true;
        }
      }
    }
    return false;
  };

  const hover = (f: Vec2, m: Mods, ctx: CanvasCtx): boolean => {
    const pp = P.current;
    if (!pp.design) return false;
    if (pp.tool === "text" || pp.tool === "barcode") {
      const hit = objOf(ctx.hitObject(f));
      if ((pp.tool === "text" && isText(hit)) || (pp.tool === "barcode" && hit && barcodeParams(hit))) ctx.showSnap(null);
      else ctx.showSnap(ctx.snap(f, m).hit);
      return true;
    }
    return false;
  };

  const overlay = (r: RenderCtx): ReactNode => (
    <TextOverlay
      r={r}
      doc={doc}
      fonts={fonts}
      tool={tool}
      design={design}
      editor={editor}
      areaRef={areaRef}
      defaults={defaults}
      barValues={barValues}
      barEditing={!!barTextObj}
      pickPath={pickPath ? (objOf(pickPath)?.name ?? pickPath) : null}
      onCancelPick={() => setPickPath(null)}
      onChange={(value) => editorRef.current && setEditor({ ...editorRef.current, value })}
      onCommit={commitEditor}
      onFinish={() => {
        commitEditor();
        if (P.current.tool === "text") P.current.setTool("select");
      }}
      onBar={barChange}
      onReloadFonts={loadFonts}
    />
  );

  const wrap = (inner: CanvasTool): CanvasTool => ({
    down: (f, m, info, ctx) => {
      if (down(f, m, info, ctx)) {
        owner.current = "text";
        return true;
      }
      owner.current = "inner";
      return inner.down(f, m, info, ctx);
    },
    drag: (f, m, ctx) => {
      if (owner.current !== "text") inner.drag(f, m, ctx);
    },
    up: (f, m, ctx) => {
      if (owner.current !== "text") inner.up(f, m, ctx);
      owner.current = null;
    },
    cancel: () => {
      if (owner.current !== "text") inner.cancel();
      owner.current = null;
    },
    hover: (f, m, ctx) => hover(f, m, ctx) || inner.hover(f, m, ctx),
    render: (r) => inner.render(r),
    overlay: (r) => (
      <>
        {inner.overlay(r)}
        {overlay(r)}
      </>
    ),
    snapParts: inner.snapParts,
  });

  // ---- toolbar ----
  const toggleTool = (x: Tool) => p.setTool(tool === x ? "select" : x);
  const toolbar = design ? (
    <RibbonSlot group="create" order={20}>
      <button
        type="button"
        aria-label={t("text.tool")}
        className={`tool-btn${tool === "text" ? " active" : ""}`}
        onClick={() => toggleTool("text")}
        disabled={!canText}
        aria-pressed={tool === "text"}
        title={canText ? t("text.toolTitle") : t("text.unsupported")}
      >
        <Icon name="text" size={20} />
        <span className="tool-caption">{t("text.tool")}</span>
      </button>
      <button
        type="button"
        aria-label={t("barcode.tool")}
        className={`tool-btn${tool === "barcode" ? " active" : ""}`}
        onClick={() => toggleTool("barcode")}
        disabled={!canBarcode}
        aria-pressed={tool === "barcode"}
        title={canBarcode ? t("barcode.toolTitle") : t("barcode.unsupported")}
      >
        <Icon name="barcode" size={20} />
        <span className="tool-caption">{t("barcode.tool")}</span>
      </button>
    </RibbonSlot>
  ) : null;

  // ---- properties ----
  const common = {
    fonts,
    run: p.run,
    report: p.report,
    notify: p.notify,
    has: p.has,
    onReloadFonts: loadFonts,
    onPickPath: (id: string) => {
      setPickPath(id);
      if (tool !== "select" && tool !== "text") p.setTool("select");
    },
    onOpenVariables: () => setVarOpen(true),
    onEditText: editText,
  };
  const varShown = !!doc && (varOpen || usesVariables(doc));
  const properties =
    design && doc ? (
      <>
        {singleText && <TextSection {...common} doc={doc} o={singleText} />}
        {singleBarcode && <BarcodeSection doc={doc} id={singleBarcode.id} run={p.run} report={p.report} />}
        {pair && p.has(TXT.applyPath) && <ApplyPathCard text={pair.text} path={pair.path} run={p.run} report={p.report} />}
        {varShown && <VarTextWindow doc={doc} has={p.has} run={p.run} report={p.report} notify={p.notify} onClose={varOpen ? () => setVarOpen(false) : undefined} />}
      </>
    ) : null;

  // ---- dialogs ----
  const dlgObj = barcodeDlg?.mode === "edit" ? objOf(barcodeDlg.id) : null;
  const dialogs = barcodeDlg ? (
    <BarcodeDialog
      key={barcodeDlg.mode === "edit" ? barcodeDlg.id : "new"}
      title={barcodeDlg.mode === "edit" ? t("barcode.editTitle") : t("barcode.createTitle")}
      okLabel={barcodeDlg.mode === "edit" ? t("barcode.apply") : t("barcode.create")}
      initial={barcodeDlg.mode === "edit" ? (barcodeParams(dlgObj) ?? lastBarcode) : lastBarcode}
      onClose={() => setBarcodeDlg(null)}
      onOk={(v) => {
        const d = barcodeDlg;
        setBarcodeDlg(null);
        setLastBarcode(v);
        const pp = P.current;
        if (d.mode === "edit") {
          void pp.run(barcodeRedraw(d.id, v)).catch(report);
          return;
        }
        void pp
          .run(barcodeCreate(v, d.at, pp.activeLayer))
          .then((r) => {
            const ids = createdIds(r);
            if (ids.length) pp.selectIds(ids);
            pp.setTool("select");
          })
          .catch(report);
      }}
    />
  ) : null;

  // ---- context menu ----
  const menuItems: MenuItem[] = [];
  if (design && doc) {
    if (singleText) menuItems.push({ label: t("text.menuEdit"), hint: t("text.menuEditHint"), action: () => editText(singleText.id) });
    if (pair && p.has(TXT.applyPath)) menuItems.push({ label: t("text.applyPath"), action: () => void p.run(applyPath(pair.text, pair.path)).catch(p.report) });
    if (singleText?.geometry.path) menuItems.push({ label: t("text.detachPath"), action: () => void p.run(detachPath(singleText.id)).catch(p.report) });
    if (singleText && !singleText.geometry.path && p.has(TXT.applyPath)) menuItems.push({ label: t("text.applyPathPick"), action: () => common.onPickPath(singleText.id) });
    if (singleBarcode) menuItems.push({ label: t("barcode.menuEdit"), action: () => editBarcode(singleBarcode.id) });
    if (p.has(TXT.setSerial)) menuItems.push({ label: t("var.menu"), action: () => setVarOpen(true) });
  }

  // ---- hotkeys ----
  const handlers: Record<string, () => boolean | void> = {
    "tool.text": () => {
      if (!design || !canText) return false;
      // T aligns the tops of two or more selected shapes (LightBurn); the text tool then waits for Ctrl+T or the toolbar.
      // In Edit Nodes, T trims at the pointer.
      if (multi || tool === "node") return false;
      p.setTool("text");
    },
    "tool.barcode": () => {
      if (!design || !canBarcode) return false;
      p.setTool("barcode");
    },
    // Command search (S, Fusion's key; components/CommandSearch.tsx). Hosted here because this hook's handlers reach
    // the key map; in Edit Nodes plain S makes a node smooth, so it declines there.
    "app.commandSearch": () => (tool === "node" || !openCommandSearch() ? false : undefined),
    "window.variableText": () => {
      if (!design) return false;
      setVarOpen((v) => !v);
    },
  };

  return { wrap, toolbar, properties, dialogs, menuItems, handlers };
}

// ---- overlay: editor, options bar, missing-font badges --------------------------------------------------------------

function TextOverlay(p: {
  r: RenderCtx;
  doc: GDocument | null;
  fonts: FontInfo[] | null;
  tool: Tool;
  design: boolean;
  editor: Editor | null;
  areaRef: React.MutableRefObject<HTMLTextAreaElement | null>;
  defaults: TextDefaults;
  barValues: TextDefaults;
  barEditing: boolean;
  pickPath: string | null;
  onCancelPick: () => void;
  onChange: (v: string) => void;
  onCommit: () => void;
  onFinish: () => void;
  onBar: (c: Partial<TextDefaults>) => void;
  onReloadFonts: () => void;
}) {
  const t = useT();
  const { r, doc, editor } = p;
  // Missing-font badges sit at the top-left of each such text.
  const badges = useMemo(() => {
    if (!doc || !p.fonts) return [];
    return doc.objects.filter((o) => isText(o) && fontMissing(o.geometry.font, p.fonts)).map((o) => ({ id: o.id, b: objectBounds(o), font: isText(o) ? o.geometry.font.family : "" }));
  }, [doc, p.fonts]);
  if (!p.design) return null;

  let edit: ReactNode = null;
  if (editor) {
    const o = editor.id ? doc?.objects.find((x) => x.id === editor.id) : null;
    const g = isText(o) ? o.geometry : null;
    const look = g ? { ...defaultsFrom(g), font: g.font } : p.defaults;
    const onPath = !!g?.path;
    const alignH = onPath ? "middle" : look.align_h;
    const alignV = onPath ? "middle" : look.align_v;
    const [tx, ty] = editorTranslate(alignH, alignV);
    const s = r.toScreen(editor.at);
    const px = Math.min(300, Math.max(12, look.height_mm * editor.scale * r.scale));
    const lines = editor.value.split("\n");
    const cols = Math.max(4, ...lines.map((l) => l.length + 2));
    // Touch (T11): iOS zooms the page into a field under 16 px. The field keeps a 16 px font and is scaled down to the
    // canvas size instead (scale before translate, so the alignment offset is in the visible size).
    const coarse = typeof document !== "undefined" && document.documentElement.dataset.pointer === "coarse";
    const fontPx = coarse ? Math.max(16, px) : px;
    const k = px / fontPx;
    edit = (
      <textarea
        ref={p.areaRef}
        className="vp-overlay tx-editor"
        autoFocus
        spellCheck={false}
        wrap="off"
        aria-label={t("text.editorLabel")}
        placeholder={t("text.editorPlaceholder")}
        value={editor.value}
        rows={Math.max(1, lines.length)}
        style={{
          left: s[0],
          top: s[1],
          fontFamily: look.font ? cssFontFamily(look.font) : "sans-serif",
          fontSize: fontPx,
          fontWeight: look.bold ? 700 : 400,
          fontStyle: look.italic ? "italic" : "normal",
          textTransform: look.upper ? "uppercase" : "none",
          textAlign: cssTextAlign(alignH),
          width: `${cols}ch`,
          transform: `rotate(${-editor.rot}deg)${k !== 1 ? ` scale(${k})` : ""} translate(${tx}%, ${ty}%)`,
        }}
        onChange={(e) => p.onChange(e.target.value)}
        onBlur={(e) => {
          // Moving into the options bar (font, height, ...) keeps editing; anything else finishes.
          if ((e.relatedTarget as Element | null)?.closest?.(".tx-bar")) return;
          p.onCommit();
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape" || (e.key === "Enter" && (e.ctrlKey || e.metaKey))) {
            e.preventDefault();
            p.onFinish();
          }
        }}
        onPointerDown={(e) => e.stopPropagation()}
      />
    );
  }

  let bar: ReactNode = null;
  if (p.pickPath) {
    bar = (
      <div className="vp-overlay et-bar tx-bar" role="toolbar" aria-label={t("text.applyPath")}>
        <span>
          <b>{t("text.applyPath")}</b>: {t("text.pickPathHint", { name: p.pickPath })}
        </span>
        <button className="btn" onClick={p.onCancelPick}>
          {t("common.cancel")}
        </button>
      </div>
    );
  } else if (p.tool === "text" || (editor && p.tool === "select")) {
    bar = <OptionsBar values={p.barValues} editingText={p.barEditing} fonts={p.fonts} onChange={p.onBar} onReloadFonts={p.onReloadFonts} />;
  } else if (p.tool === "barcode") {
    bar = (
      <div className="vp-overlay et-bar tx-bar" role="toolbar" aria-label={t("barcode.tool")}>
        <span>
          <b>{t("barcode.tool")}</b>: {t("barcode.barHint")}
        </span>
      </div>
    );
  }

  return (
    <>
      {badges.map((x) => {
        if (!x.b) return null;
        const s = r.toScreen([x.b.x0, x.b.y1]);
        if (s[0] < -40 || s[1] < -20 || s[0] > r.size[0] || s[1] > r.size[1] + 20) return null;
        return (
          <span key={x.id} className="vp-overlay tx-badge warn tx-canvas-badge" style={{ left: s[0], top: s[1] }} title={t("text.missingFontTitle", { font: x.font })}>
            {t("text.missing")}
          </span>
        );
      })}
      {edit}
      {/* The Text Options bar goes to the context strip itself; Apply to Path and Barcode float in the canvas HUD. */}
      {bar && (
        <HudItem id="text-bar" slot="tl" rank={21} minW={200}>
          {bar}
        </HudItem>
      )}
    </>
  );
}

/** LightBurn's Text Options toolbar while the Text tool is active. */
function OptionsBar(p: { values: TextDefaults; editingText: boolean; fonts: FontInfo[] | null; onChange: (c: Partial<TextDefaults>) => void; onReloadFonts: () => void }) {
  const t = useT();
  const v = p.values;
  const al = alignOptions(t);
  const toggle = (k: "bold" | "italic" | "upper" | "welded", label: string, title: string) => (
    <button type="button" className={`tx-toggle${v[k] ? " on" : ""}`} aria-pressed={v[k]} title={title} onClick={() => p.onChange({ [k]: !v[k] })}>
      {label}
    </button>
  );
  // In the context strip like the other tools' bars (plan V16), not floating in two rows over the rulers and the
  // canvas top; where no strip is mounted (the phone) it stays over the canvas.
  return (
    <ContextStripPortal>
    <div className="vp-overlay et-bar tx-bar" role="toolbar" aria-label={t("text.options")}>
      <b>{p.editingText ? t("text.optionsSelected") : t("text.optionsNew")}</b>
      <FontPicker compact value={v.font} fonts={p.fonts} onPick={(font) => p.onChange({ font })} onReload={p.onReloadFonts} placeholder={t("text.font.default")} />
      <BarNumber label={t("text.height")} unit="mm" value={v.height_mm} min={0.01} onCommit={(height_mm) => p.onChange({ height_mm })} />
      <BarNumber label={t("text.hSpaceShort")} unit="%" value={v.h_spacing} min={-100} max={1000} onCommit={(h_spacing) => p.onChange({ h_spacing })} />
      <BarNumber label={t("text.vSpaceShort")} unit="%" value={v.v_spacing} min={-100} max={1000} onCommit={(v_spacing) => p.onChange({ v_spacing })} />
      <Segmented label={t("text.alignX")} value={v.align_h} options={al.h} onChange={(align_h) => p.onChange({ align_h })} />
      <Segmented label={t("text.alignY")} value={v.align_v} options={al.v} onChange={(align_v) => p.onChange({ align_v })} />
      <span className="tx-toggles">
        {toggle("bold", "B", t("text.bold"))}
        {toggle("italic", "I", t("text.italic"))}
        {toggle("upper", "AA", t("text.upper"))}
        {toggle("welded", "W", t("text.welded"))}
      </span>
      <span className="et-hint">{t("text.barHint")}</span>
    </div>
    </ContextStripPortal>
  );
}

/** A compact number box for the options bar, in the display unit for mm. */
function BarNumber(p: { label: string; unit: string; value: number; min?: number; max?: number; onCommit: (v: number) => void }) {
  const conv = useUnitConversion(p.unit);
  const shown = trimNumber(conv.to(p.value), 3 + conv.extraDigits);
  const [text, setText] = useState<string | null>(null);
  const commit = () => {
    if (text === null) return;
    const n = Number(text.replace(",", "."));
    setText(null);
    if (!Number.isFinite(n)) return;
    let mm = conv.from(n);
    if (p.min !== undefined) mm = Math.max(p.min, mm);
    if (p.max !== undefined) mm = Math.min(p.max, mm);
    if (Math.abs(mm - p.value) > 1e-9) p.onCommit(mm);
  };
  return (
    <label className="et-opt tx-num">
      {p.label}
      <input
        type="text"
        inputMode="decimal"
        value={text ?? shown}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setText(null);
        }}
      />
      <span className="unit">{conv.unit}</span>
    </label>
  );
}
