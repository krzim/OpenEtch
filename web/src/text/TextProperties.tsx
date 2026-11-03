// Properties for text and barcodes (LightBurn's Text Options toolbar and Shape Properties for a text object, and Bar
// Code Properties): every field commits one engine command, `text.update` (or `text.apply_path`, `text.detach_path`,
// `text.refresh`, `text.convert_to_path`) for text and `shape.barcode {id}` for a barcode.

import { useEffect, useRef, useState } from "react";
import type { Invocation } from "../api/client.ts";
import type { GDocument, TextAlignH, TextAlignV } from "../api/types.ts";
import { CheckField, NumberField } from "../components/ui.tsx";
import { useT } from "../workspace/usePrefs.ts";
import { BarcodeForm } from "./Barcode.tsx";
import { FontPicker } from "./FontPicker.tsx";
import { FitSection } from "./TextFit.tsx";
import {
  TEXT_MODES,
  applyPath,
  barcodeParams,
  barcodeRedraw,
  convertText,
  detachPath,
  fontInfo,
  fontMissing,
  normalizeText,
  refreshText,
  shownText,
  textMode,
  textUpdate,
  type FontInfo,
  type TextFields,
  type TextMode,
  type TextObject,
} from "./textModel.ts";
import { VarHelp } from "./VarHelp.tsx";
import { HelpLink } from "../guide/HelpLink.tsx";

type Run = (inv: Invocation | Invocation[]) => Promise<unknown>;

export interface TextPropsCommon {
  doc: GDocument;
  fonts: FontInfo[] | null;
  run: Run;
  report: (e: unknown) => void;
  notify: (msg: string, kind?: "info" | "error") => void;
  has: (cmd: string) => boolean;
  onReloadFonts: () => void;
  onPickPath: (textId: string) => void;
  onOpenVariables: () => void;
  onEditText: (id: string) => void;
}

/** Engine warnings of a text command (missing glyphs or faces), shown as a toast. */
export function warnFrom(r: unknown, notify: (m: string, k?: "info" | "error") => void): void {
  const w = r && typeof r === "object" ? (r as { warnings?: unknown }).warnings : null;
  if (Array.isArray(w) && w.length) notify(w.filter((x) => typeof x === "string").join("; "));
}

/** Align X / Align Y as three toggle buttons. */
export function Segmented<V extends string>(p: { label: string; value: V; options: [V, string, string][]; onChange: (v: V) => void }) {
  return (
    <div className="field">
      <span className="field-label">{p.label}</span>
      <div className="tx-seg" role="radiogroup" aria-label={p.label}>
        {p.options.map(([v, glyph, title]) => (
          <button key={v} type="button" role="radio" aria-checked={p.value === v} className={p.value === v ? "on" : ""} title={title} onClick={() => p.value !== v && p.onChange(v)}>
            {glyph}
          </button>
        ))}
      </div>
    </div>
  );
}

export function alignOptions(t: ReturnType<typeof useT>): { h: [TextAlignH, string, string][]; v: [TextAlignV, string, string][] } {
  return {
    h: [
      ["left", "⇤", t("text.align.left")],
      ["middle", "↔", t("text.align.middle")],
      ["right", "⇥", t("text.align.right")],
    ],
    v: [
      ["top", "⤒", t("text.align.top")],
      ["middle", "↕", t("text.align.middle")],
      ["bottom", "⤓", t("text.align.bottom")],
    ],
  };
}

/** The missing-font badge and what to do about it. */
export function MissingFont(p: { family: string }) {
  const t = useT();
  return (
    <p className="tx-missing" role="status">
      <span className="tx-badge warn">{t("text.missing")}</span> {t("text.missingFont", { font: p.family })}
    </p>
  );
}

/** The text (several lines) with a commit on blur or Ctrl+Enter. The browser's spell check is on for plain text
 * (Fider #817); a variable-text format (serial and date codes) is not words, so it is off there. */
function TextBody(p: { value: string; onCommit: (v: string) => void; label: string; spellCheck: boolean }) {
  const [text, setText] = useState(p.value);
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(p.value);
  }, [p.value]);
  return (
    <label className="field tx-stack">
      <span className="field-label">{p.label}</span>
      <textarea
        className="tx-area"
        rows={Math.min(6, Math.max(2, text.split("\n").length))}
        value={text}
        spellCheck={p.spellCheck}
        onFocus={() => (editing.current = true)}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          editing.current = false;
          const v = normalizeText(text);
          if (v !== p.value) {
            if (v.trim()) p.onCommit(v);
            else setText(p.value);
          }
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) (e.target as HTMLTextAreaElement).blur();
          if (e.key === "Escape") {
            setText(p.value);
            editing.current = false;
            requestAnimationFrame(() => (e.target as HTMLTextAreaElement).blur());
          }
        }}
      />
    </label>
  );
}

/** Bend slider: moves freely while dragged and sends one edit when released (one undo step, not one per pixel). */
function BendSlider(p: { value: number; label: string; disabled?: boolean; onCommit: (v: number) => void }) {
  const [drag, setDrag] = useState<number | null>(null);
  const commit = () => {
    if (drag !== null && drag !== p.value) p.onCommit(drag);
    setDrag(null);
  };
  return (
    <input
      className="tx-range"
      type="range"
      min={-360}
      max={360}
      step={1}
      value={drag ?? p.value}
      aria-label={p.label}
      disabled={p.disabled}
      onChange={(e) => setDrag(Number(e.target.value))}
      onPointerUp={commit}
      onKeyUp={commit}
      onBlur={commit}
      onDoubleClick={() => p.value !== 0 && p.onCommit(0)}
      title={p.label}
    />
  );
}

export function TextSection(p: TextPropsCommon & { o: TextObject }) {
  const t = useT();
  const { o, run, report, notify } = p;
  const g = o.geometry;
  const up = (f: TextFields) =>
    run(textUpdate(o.id, f))
      .then((r) => warnFrom(r, notify))
      .catch(report);
  const mode = textMode(g);
  const info = fontInfo(g.font, p.fonts);
  const missing = fontMissing(g.font, p.fonts);
  const pathObj = g.path ? p.doc.objects.find((x) => x.id === g.path?.object) : undefined;
  const al = alignOptions(t);
  const isStroke = g.font.kind !== "system";
  return (
    <section className="prop-section tx-section" aria-label={t("text.title")}>
      <h3>
        {t("text.title")}
        <HelpLink to="text#text-properties" topic={t("text.title")} />
        {missing && <span className="tx-badge warn">{t("text.missing")}</span>}
        {mode !== "normal" && <span className="tx-badge">{t("text.variableBadge")}</span>}
      </h3>
      {missing && <MissingFont family={g.font.family} />}
      <TextBody label={mode === "normal" ? t("text.text") : t("text.format")} value={g.text} spellCheck={mode === "normal"} onCommit={(text) => void up({ text })} />
      {g.shown && g.shown !== g.text && (
        <p className="tx-shown">
          <span className="field-label">{t("text.shows")}</span> <span className="tx-shown-v">{shownText(g)}</span>
        </p>
      )}
      <div className="tx-row-btns">
        <button className="btn" onClick={() => p.onEditText(o.id)} title={t("text.editOnCanvasTitle")}>
          {t("text.editOnCanvas")}
        </button>
      </div>
      <div className="field">
        <span className="field-label">{t("text.font")}</span>
        <FontPicker value={g.font} fonts={p.fonts} sample={mode === "normal" ? shownText(g) : undefined} onPick={(font) => void up({ font })} onReload={p.onReloadFonts} />
      </div>
      <NumberField label={t("text.height")} unit="mm" value={g.height_mm} min={0.01} max={10000} digits={3} onCommit={(v) => v !== null && v > 0 && void up({ height_mm: v })} />
      <div className="field-grid">
        <NumberField label={t("text.hSpace")} unit="%" value={g.h_spacing} min={-100} max={1000} digits={2} onCommit={(v) => v !== null && void up({ h_spacing: v })} />
        <NumberField label={t("text.vSpace")} unit="%" value={g.v_spacing} min={-100} max={1000} digits={2} onCommit={(v) => v !== null && void up({ v_spacing: v })} />
      </div>
      <Segmented label={t("text.alignX")} value={g.align_h} options={al.h} onChange={(align_h) => void up({ align_h })} />
      <Segmented label={t("text.alignY")} value={g.align_v} options={al.v} onChange={(align_v) => void up({ align_v })} />
      <div className="tx-checks">
        <CheckField label={t("text.bold") + (info && !info.bold && !isStroke ? ` ${t("text.noFace")}` : "")} checked={g.bold} onChange={(bold) => void up({ bold })} />
        <CheckField label={t("text.italic")} checked={g.italic} onChange={(italic) => void up({ italic })} />
        <CheckField label={t("text.upper")} checked={g.upper} onChange={(upper) => void up({ upper })} />
        <CheckField label={t("text.welded")} checked={g.welded} onChange={(welded) => void up({ welded })} />
      </div>
      {isStroke && <p className="hint">{t("text.strokeHint")}</p>}

      <h4>{t("text.bend")} <HelpLink to="text#bend-text" topic={t("text.bend")} /></h4>
      <div className="field">
        <span className="field-label">{t("text.bendAngle")}</span>
        <BendSlider value={g.bend_deg} label={t("text.bendAngle")} disabled={!!g.path} onCommit={(bend_deg) => void up({ bend_deg })} />
      </div>
      <div className="field-grid">
        <NumberField label="°" value={g.bend_deg} min={-360} max={360} digits={1} onCommit={(v) => v !== null && void up({ bend_deg: v })} disabled={!!g.path} />
        <CheckField label={t("text.distort")} checked={g.distort} onChange={(distort) => void up({ distort })} />
      </div>

      <h4>{t("text.maxWidthHead")} <HelpLink to="text#max-width-and-squeeze" topic={t("text.maxWidthHead")} /></h4>
      <div className="field-grid">
        <NumberField label={t("text.maxWidth")} unit="mm" value={g.max_width_mm} min={0} digits={3} placeholder={t("text.none")} onCommit={(v) => v !== null && void up({ max_width_mm: v })} />
        <CheckField label={t("text.squeeze")} checked={g.squeeze} onChange={(squeeze) => void up({ squeeze })} />
      </div>

      <FitSection o={o} doc={p.doc} run={run} report={report} onWarnings={(r) => warnFrom(r, notify)} />

      <h4>{t("text.pathHead")} <HelpLink to="text#text-on-a-path" topic={t("text.pathHead")} /></h4>
      {g.path ? (
        <>
          <p className="hint">{t("text.onPath", { name: pathObj?.name ?? g.path.object })}</p>
          <NumberField label={t("text.pathOffset")} unit="mm" value={g.path.offset_mm} digits={3} onCommit={(v) => v !== null && void up({ path_offset_mm: v })} />
          <div className="tx-row-btns">
            <button className="btn" onClick={() => void run(refreshText([o.id])).then((r) => warnFrom(r, notify)).catch(report)} title={t("text.followPathTitle")}>
              {t("text.followPath")}
            </button>
            <button className="btn" onClick={() => void run(detachPath(o.id)).catch(report)}>
              {t("text.detachPath")}
            </button>
          </div>
        </>
      ) : (
        <div className="tx-row-btns">
          <button className="btn" onClick={() => p.onPickPath(o.id)} title={t("text.applyPathTitle")}>
            {t("text.applyPathPick")}
          </button>
        </div>
      )}

      <h4>{t("text.variableHead")} <HelpLink to="variable-text-and-barcodes#text-modes" topic={t("text.variableHead")} /></h4>
      <label className="field">
        <span className="field-label">{t("text.mode")}</span>
        <span className="field-input">
          <select value={mode} onChange={(e) => void up({ variable: { mode: e.target.value as TextMode, offset: g.variable?.offset ?? 0 } })}>
            {TEXT_MODES.map((m) => (
              <option key={m.id} value={m.id}>
                {t(m.key)}
              </option>
            ))}
          </select>
        </span>
      </label>
      {mode !== "normal" && (
        <>
          <NumberField label={t("text.offset")} value={g.variable?.offset ?? 0} step={1} digits={0} onCommit={(v) => v !== null && void up({ variable: { mode, offset: Math.round(v) } })} />
          <VarHelp mode={mode} />
          {(mode === "serial" || mode === "csv") && (
            <div className="tx-row-btns">
              <button className="btn" onClick={p.onOpenVariables}>
                {t("var.open")}
              </button>
            </div>
          )}
        </>
      )}
      <CheckField label={t("text.ignoreEmpty")} checked={g.ignore_empty_vars} onChange={(ignore_empty_vars) => void up({ ignore_empty_vars })} />

      <div className="tx-row-btns tx-foot">
        <button className="btn" onClick={() => void run(refreshText([o.id])).then((r) => warnFrom(r, notify)).catch(report)} title={t("text.refreshTitle")}>
          {t("text.refresh")}
        </button>
        <button className="btn" onClick={() => void run(convertText([o.id])).catch(report)} title={t("text.convertTitle")}>
          {t("menu.convertToPath")}
        </button>
      </div>
    </section>
  );
}

/** Bar Code Properties of a selected barcode. */
export function BarcodeSection(p: Pick<TextPropsCommon, "run" | "report"> & { id: string; doc: GDocument }) {
  const t = useT();
  const o = p.doc.objects.find((x) => x.id === p.id);
  const params = barcodeParams(o);
  if (!o || !params) return null;
  return (
    <section className="prop-section tx-section" aria-label={t("barcode.title")}>
      <h3>{t("barcode.title")} <HelpLink to="variable-text-and-barcodes#barcodes-and-qr-codes" topic={t("barcode.title")} /></h3>
      <BarcodeForm live value={params} onChange={(v) => void p.run(barcodeRedraw(o.id, v)).catch(p.report)} />
    </section>
  );
}

/** A text and a vector shape selected together: LightBurn's Apply Path to Text. */
export function ApplyPathCard(p: Pick<TextPropsCommon, "run" | "report"> & { text: string; path: string }) {
  const t = useT();
  return (
    <section className="prop-section tx-section">
      <h3>{t("text.applyPath")} <HelpLink to="text#text-on-a-path" topic={t("text.applyPath")} /></h3>
      <p className="hint">{t("text.applyPathHint")}</p>
      <button className="btn btn-primary" onClick={() => void p.run(applyPath(p.text, p.path)).catch(p.report)}>
        {t("text.applyPath")}
      </button>
    </section>
  );
}
