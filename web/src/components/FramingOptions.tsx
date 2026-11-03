// Framing (LightBurn's Galvo Framing, light only): the Frame and Stop frame buttons with an options popover: style
// (Bounds, Hull, Contour), Frame individually, Outside shapes only, Tool layers only, Selection only, once or
// continuous, frame speed, red pointer / focus light / both, blanking and Live framing. With Live framing on, the frame
// follows every edit, and while it runs the keyboard moves, scales and rotates the framed shapes as in LightBurn
// (arrows 5 mm, Shift 20 mm, Ctrl 0.5 mm; Page Up/Down scale; . and , rotate 15°, Shift 1.5°, Ctrl 45°), each key one
// `object.transform`. The engine's framing code path can never open the laser.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import type { Invocation } from "../api/client.ts";
import * as C from "../api/commands.ts";
import { frameKeyEdit, frameRequestBody, type FrameOptions, type FramePointer, type FrameStyle } from "../api/job.ts";
import { jobApi } from "../api/jobApi.ts";
import type { GDocument, MachineStatus } from "../api/types.ts";
import { useT } from "../workspace/usePrefs.ts";
import { getFrameOptions, setFrameOptions, useFrameOptions } from "./jobStore.ts";
import { useEscStopsFraming } from "../laser/escFraming.ts";
import { placePopover, type Rect } from "../laser/placePopover.ts";
import { Icon, NumberField } from "./ui.tsx";
import "./job.css";
import { errorText } from "../api/errorText.ts";

type Notify = (msg: string, kind?: "info" | "error") => void;

/** Start framing with the stored options (also the framing hotkeys). Throws a user-facing Error when impossible. */
export async function frameWithOptions(selection: readonly string[], override: { style?: FrameStyle; selectionOnly?: boolean } = {}): Promise<void> {
  const o = { ...getFrameOptions(), ...override };
  await jobApi.frame(frameRequestBody(o, selection));
}

/** The options a running frame was last re-traced with (several Frame controls may be mounted: the dock and the
 * tablet's context strip; only one re-traces per change). */
let reframedFor: FrameOptions | null = null;

function rectOf(el: Element): Rect {
  const b = el.getBoundingClientRect();
  return { left: b.left, top: b.top, right: b.right, bottom: b.bottom };
}

/** What the frame popover must never cover (V13): every visible Start and STOP. */
function startStopRects(): Rect[] {
  return [...document.querySelectorAll(".btn-start, .btn-stop")].map(rectOf).filter((r) => r.right > r.left && r.bottom > r.top);
}

/** Whether a key event belongs to a text field (those keep their keys). */
function typing(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export function FramingControls(p: {
  machine: MachineStatus | null;
  doc: GDocument | null;
  selection: readonly string[];
  designOnly: string | null;
  run: (inv: Invocation | Invocation[]) => Promise<unknown>;
  notify: Notify;
  report: (e: unknown) => void;
  onChanged: () => void;
  /** The tablet's context strip: no summary text. */
  compact?: boolean;
}) {
  const t = useT();
  const o = useFrameOptions();
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  const caret = useRef<HTMLButtonElement>(null);
  const [place, setPlace] = useState<CSSProperties>({ visibility: "hidden" });
  const framing = !!p.machine?.framing;

  // Close the popover on an outside press or Escape (Escape closes only the popover: it never reaches the shell, and
  // never stops the frame while the popover is open).
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      const n = e.target as Node;
      if (box.current?.contains(n) || pop.current?.contains(n)) return;
      setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      caret.current?.focus();
    };
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("keydown", key, true);
    };
  }, [open]);

  // A floating popover (docs/ux/ui-plan-2026-11.md P5, V13): below the Frame button, else above, else beside the
  // panel towards the canvas; inside the window with a max-height, never over Start or STOP. Placed again when the
  // window or a scroll container moves the button.
  const placeNow = useCallback(() => {
    const el = pop.current;
    const anchorEl = box.current;
    if (!el || !anchorEl) return;
    const panel = anchorEl.closest(".panel, .dock, aside, .context-strip-slot") ?? anchorEl;
    const natural = { w: el.offsetWidth, h: el.scrollHeight + 2 };
    const at = placePopover(rectOf(anchorEl), natural, { w: window.innerWidth, h: window.innerHeight }, startStopRects(), { sideOf: rectOf(panel) });
    setPlace({ left: at.left, top: at.top, maxHeight: at.maxHeight });
  }, []);
  useLayoutEffect(() => {
    if (!open) {
      setPlace({ visibility: "hidden" });
      return;
    }
    placeNow();
    pop.current?.focus({ preventScroll: true });
    const again = () => placeNow();
    window.addEventListener("resize", again);
    window.addEventListener("scroll", again, true);
    return () => {
      window.removeEventListener("resize", again);
      window.removeEventListener("scroll", again, true);
    };
  }, [open, placeNow]);
  // The content changes height with some options (Live framing adds a hint): place it again.
  useLayoutEffect(() => {
    if (open) placeNow();
  }, [open, o, placeNow]);

  // Esc stops framing (owner decision 4; never a burn). The status chip installs the same key; each press stops once.
  const afterEscStop = useCallback(
    (err: unknown | null) => {
      if (err) ctx.current.report(err);
      else ctx.current.notify(t("frame.stoppedToast"));
      ctx.current.onChanged();
    },
    [t],
  );
  useEscStopsFraming(framing, afterEscStop);

  // Live framing: keys move, scale and rotate what is framed (the selection, else every shape that is output).
  const ctx = useRef(p);
  ctx.current = p;
  useEffect(() => {
    if (!framing || !o.live) return;
    const onKey = (e: KeyboardEvent) => {
      if (typing(e.target) || e.altKey || document.querySelector(".modal-backdrop")) return;
      const edit = frameKeyEdit(e.key, { shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey });
      if (!edit) return;
      const { doc, selection, run, report } = ctx.current;
      if (!doc) return;
      const out = new Set(doc.layers.filter((l) => l.output).map((l) => l.id));
      const ids = selection.length ? [...selection] : doc.objects.filter((x) => out.has(x.layer)).map((x) => x.id);
      if (ids.length === 0) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const tr = "translate" in edit ? { translate: edit.translate } : "scale" in edit ? { scale: round6(edit.scale), about: "center" as const } : { rotate_deg: edit.rotate_deg, about: "center" as const };
      run(C.transform(ids, tr)).catch(report);
    };
    // Capture: ahead of the canvas's own nudge keys, which only work in the Design workspace.
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [framing, o.live]);

  // A frame already running follows the options: changing the style (or any other option) while framing traces the
  // frame again with the new options, as the Frame button would.
  const shown = useRef(o);
  useEffect(() => {
    const before = shown.current;
    shown.current = o;
    if (!framing || before === o || reframedFor === o) return;
    reframedFor = o;
    try {
      jobApi.frame(frameRequestBody(o, ctx.current.selection)).catch(ctx.current.report).finally(ctx.current.onChanged);
    } catch (e) {
      ctx.current.notify(errorText(e), "error");
    }
  }, [o, framing]);

  const start = () => {
    try {
      const body = frameRequestBody(o, p.selection);
      jobApi
        .frame(body)
        .then(() => p.notify(o.live ? t("frame.liveToast") : t("frame.toast")))
        .catch(p.report)
        .finally(p.onChanged);
    } catch (e) {
      p.notify(errorText(e), "error");
    }
  };
  const stop = () =>
    void jobApi
      .frameStop()
      .then(() => p.notify(t("frame.stoppedToast")))
      .catch(p.report)
      .finally(p.onChanged);

  const styles: { v: FrameStyle; label: string; title: string }[] = [
    { v: "bounds", label: t("frame.bounds"), title: t("frame.boundsTitle") },
    { v: "hull", label: t("frame.hull"), title: t("frame.hullTitle") },
    { v: "outline", label: t("frame.contour"), title: t("frame.contourTitle") },
  ];
  const pointers: { v: FramePointer; label: string }[] = [
    { v: "red", label: t("frame.red") },
    { v: "focus", label: t("frame.focus") },
    { v: "both", label: t("frame.both") },
  ];
  const selEmpty = o.selectionOnly && p.selection.length === 0;
  return (
    <div className="frame-ctl" ref={box}>
      <div className="frame-split">
        <button className="btn" disabled={!!p.designOnly || selEmpty} title={p.designOnly ?? (selEmpty ? t("laser.selectFirst") : t("laser.frameTitle"))} onClick={start}>
          <Icon name="frame" size={18} /> {t("laser.frame")}
        </button>
        <button
          className={`btn frame-caret${open ? " active" : ""}`}
          ref={caret}
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-label={t("frame.options")}
          title={t("frame.options")}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevronDown" size={14} />
        </button>
      </div>
      <button className="btn" onClick={stop} title={t("laser.frameStopTitle")}>
        <Icon name="frameStop" size={18} /> {t("laser.frameStop")}
      </button>
      {!p.compact && (
        <span className="frame-summary muted">
          {styles.find((s) => s.v === o.style)?.label}
          {o.live ? ` · ${t("frame.live")}` : ""}
          {o.selectionOnly ? ` · ${t("laser.selectionOnly")}` : ""}
        </span>
      )}
      {open &&
        createPortal(
        <div className="frame-pop" role="dialog" aria-label={t("frame.options")} ref={pop} tabIndex={-1} style={place}>
          <div className="frame-pop-h">{t("frame.style")}</div>
          <div className="segmented" role="radiogroup" aria-label={t("frame.style")}>
            {styles.map((s) => (
              <button key={s.v} role="radio" aria-checked={o.style === s.v} className={o.style === s.v ? "active" : ""} title={s.title} onClick={() => setFrameOptions({ style: s.v })}>
                {s.label}
              </button>
            ))}
          </div>
          <label className="laser-check" title={t("frame.individuallyTitle")}>
            <input type="checkbox" checked={o.individually} disabled={o.style === "outline"} onChange={(e) => setFrameOptions({ individually: e.target.checked })} />
            {t("frame.individually")}
          </label>
          <label className="laser-check" title={t("frame.outsideTitle")}>
            <input type="checkbox" checked={o.outsideOnly} disabled={o.style !== "outline"} onChange={(e) => setFrameOptions({ outsideOnly: e.target.checked })} />
            {t("frame.outside")}
          </label>
          <label className="laser-check" title={t("frame.imageBoundsTitle")}>
            <input type="checkbox" checked={o.imageBounds === true} onChange={(e) => setFrameOptions({ imageBounds: e.target.checked })} />
            {t("frame.imageBounds")}
          </label>
          <label className="laser-check" title={t("laser.selectionTitle")}>
            <input type="checkbox" checked={o.selectionOnly} onChange={(e) => setFrameOptions({ selectionOnly: e.target.checked })} />
            {t("laser.selectionOnly")}
          </label>
          <label className="laser-check" title={t("frame.toolLayersTitle")}>
            <input type="checkbox" checked={o.toolLayersOnly} disabled={o.selectionOnly} onChange={(e) => setFrameOptions({ toolLayersOnly: e.target.checked })} />
            {t("frame.toolLayers")}
          </label>

          <div className="frame-pop-h">{t("frame.trace")}</div>
          <div className="segmented" role="radiogroup" aria-label={t("frame.trace")}>
            <button role="radio" aria-checked={!o.continuous} className={!o.continuous ? "active" : ""} onClick={() => setFrameOptions({ continuous: false })}>
              {t("frame.once")}
            </button>
            <button role="radio" aria-checked={o.continuous} className={o.continuous ? "active" : ""} onClick={() => setFrameOptions({ continuous: true })}>
              {t("frame.continuous")}
            </button>
          </div>
          <NumberField
            label={t("frame.speed")}
            unit="mm/s"
            value={o.speedMmS}
            allowEmpty
            placeholder={t("frame.deviceDefault")}
            min={1}
            digits={0}
            onCommit={(v) => setFrameOptions({ speedMmS: v })}
          />

          <div className="frame-pop-h">{t("frame.pointer")}</div>
          <div className="segmented" role="radiogroup" aria-label={t("frame.pointer")}>
            {pointers.map((x) => (
              <button key={x.v} role="radio" aria-checked={o.pointer === x.v} className={o.pointer === x.v ? "active" : ""} title={x.v !== "red" ? t("frame.focusTitle") : undefined} onClick={() => setFrameOptions({ pointer: x.v })}>
                {x.label}
              </button>
            ))}
          </div>
          <label className="field">
            <span className="field-label">{t("frame.blanking")}</span>
            <span className="field-input">
              <select
                value={o.blanking === null ? "" : o.blanking ? "on" : "off"}
                title={t("frame.blankingTitle")}
                onChange={(e) => setFrameOptions({ blanking: e.target.value === "" ? null : e.target.value === "on" })}
              >
                <option value="">{t("frame.deviceDefault")}</option>
                <option value="on">{t("frame.on")}</option>
                <option value="off">{t("frame.off")}</option>
              </select>
            </span>
          </label>
          <label className="laser-check" title={t("frame.liveTitle")}>
            <input type="checkbox" checked={o.live} onChange={(e) => setFrameOptions({ live: e.target.checked })} />
            {t("frame.live")}
          </label>
          {o.live && <p className="hint">{t("frame.liveKeys")}</p>}
          <p className="hint">{t("frame.lightOnly")}</p>
        </div>,
          document.body,
        )}
    </div>
  );
}

function round6(v: number): number {
  return Math.round(v * 1e6) / 1e6;
}
