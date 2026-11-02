// Rulers along the top and left of the canvas, in field millimetres or inches (the display unit; +Y up, so the left
// ruler counts up), with the guides marked on them. Dragging out of a ruler makes a guide (top ruler: horizontal, left
// ruler: vertical, as in LightBurn); dropping it back on a ruler throws it away. The corner square clears all guides.

import { useRef } from "react";
import type { RenderCtx } from "../canvasTool.ts";
import { capturePointer } from "../pointer.ts";
import type { Vec2 } from "../api/types.ts";
import { rulerTicks } from "../drawTools.ts";
import { usePrefs } from "../workspace/usePrefs.ts";
import { useT } from "../workspace/usePrefs.ts";
import { Icon } from "./ui.tsx";

/** Ruler thickness, px. */
export const RULER_PX = 20;

export function Rulers(props: {
  r: RenderCtx;
  guideDrag: { axis: "x" | "y"; v: number; remove: boolean } | null;
  onDrag: (axis: "x" | "y", screen: Vec2, phase: "move" | "end" | "cancel") => void;
  guides: number;
  onClearGuides: () => void;
}) {
  const t = useT();
  const { r } = props;
  const unit = usePrefs().units;
  const [w, h] = r.size;
  const xs = rulerTicks(r.toField([RULER_PX, 0])[0], r.toField([w, 0])[0], r.scale, 60, unit);
  const ys = rulerTicks(r.toField([0, h])[1], r.toField([0, RULER_PX])[1], r.scale, 60, unit);
  /** The pointer dragging a guide out of a ruler. Tracked here rather than with hasPointerCapture: on touch the capture
   * can fail, and the guide must still follow and end (audit T17). */
  const dragging = useRef<number | null>(null);

  /** Pointer handlers for one ruler: the guide follows the pointer until release. */
  const dragHandlers = (axis: "x" | "y") => {
    const local = (e: React.PointerEvent<HTMLDivElement>): Vec2 => {
      const vp = e.currentTarget.closest(".viewport")?.getBoundingClientRect();
      return vp ? [e.clientX - vp.left, e.clientY - vp.top] : [e.clientX, e.clientY];
    };
    return {
      onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
        if (e.pointerType === "mouse" && e.button !== 0) return;
        if (dragging.current !== null) return; // a second finger
        e.preventDefault();
        dragging.current = e.pointerId;
        props.onDrag(axis, local(e), "move");
        if (capturePointer(e.currentTarget, e.pointerId)) return;
        // No capture (a touch that the browser would not capture): follow the pointer on the window instead.
        const id = e.pointerId;
        const vp = e.currentTarget.closest(".viewport");
        const at = (ev: PointerEvent): Vec2 => {
          const b = vp?.getBoundingClientRect();
          return b ? [ev.clientX - b.left, ev.clientY - b.top] : [ev.clientX, ev.clientY];
        };
        const onMove = (ev: PointerEvent) => {
          if (ev.pointerId === id && dragging.current === id) props.onDrag(axis, at(ev), "move");
        };
        const onEnd = (ev: PointerEvent) => {
          if (ev.pointerId !== id) return;
          window.removeEventListener("pointermove", onMove);
          window.removeEventListener("pointerup", onEnd);
          window.removeEventListener("pointercancel", onEnd);
          if (dragging.current !== id) return;
          dragging.current = null;
          props.onDrag(axis, at(ev), ev.type === "pointercancel" ? "cancel" : "end");
        };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onEnd);
        window.addEventListener("pointercancel", onEnd);
      },
      onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => {
        if (dragging.current !== e.pointerId) return;
        props.onDrag(axis, local(e), "move");
      },
      onPointerUp: (e: React.PointerEvent<HTMLDivElement>) => {
        if (dragging.current !== e.pointerId) return;
        dragging.current = null;
        props.onDrag(axis, local(e), "end");
      },
      onPointerCancel: (e: React.PointerEvent<HTMLDivElement>) => {
        if (dragging.current !== e.pointerId) return;
        dragging.current = null;
        props.onDrag(axis, local(e), "cancel");
      },
    };
  };

  // The top ruler makes horizontal guides (y = const) and measures x; the left one the other way round.
  const gd = props.guideDrag;
  return (
    <>
      <div className="et-ruler et-ruler-top" title={t("ru.dragDownToPlaceAHorizontal")} {...dragHandlers("y")}>
        <svg width={Math.max(0, w - RULER_PX)} height={RULER_PX}>
          {xs.map((t) => {
            const x = r.toScreen([t.v, 0])[0] - RULER_PX;
            return (
              <g key={t.v}>
                <line x1={x} x2={x} y1={t.major ? 0 : RULER_PX - 5} y2={RULER_PX} className={t.major ? "major" : "minor"} />
                {t.label !== undefined && (
                  <text x={x + 3} y={10}>
                    {t.label}
                  </text>
                )}
              </g>
            );
          })}
          {gd && gd.axis === "x" && !gd.remove && <line className="guide-mark" x1={r.toScreen([gd.v, 0])[0] - RULER_PX} x2={r.toScreen([gd.v, 0])[0] - RULER_PX} y1={0} y2={RULER_PX} />}
        </svg>
      </div>
      <div className="et-ruler et-ruler-left" title={t("ru.dragRightToPlaceAVertical")} {...dragHandlers("x")}>
        <svg width={RULER_PX} height={Math.max(0, h - RULER_PX)}>
          {ys.map((t) => {
            const y = r.toScreen([0, t.v])[1] - RULER_PX;
            return (
              <g key={t.v}>
                <line y1={y} y2={y} x1={t.major ? 0 : RULER_PX - 5} x2={RULER_PX} className={t.major ? "major" : "minor"} />
                {t.label !== undefined && (
                  <text x={10} y={y - 3} transform={`rotate(-90 10 ${y - 3})`}>
                    {t.label}
                  </text>
                )}
              </g>
            );
          })}
          {gd && gd.axis === "y" && !gd.remove && <line className="guide-mark" y1={r.toScreen([0, gd.v])[1] - RULER_PX} y2={r.toScreen([0, gd.v])[1] - RULER_PX} x1={0} x2={RULER_PX} />}
        </svg>
      </div>
      <button
        className="et-ruler-corner"
        title={props.guides ? `Clear ${props.guides} guide${props.guides === 1 ? "" : "s"}` : `Rulers in ${unit === "in" ? "inches" : "mm"}. Drag out of a ruler to place a guide.`}
        aria-label={t("ru.clearGuides")}
        disabled={!props.guides}
        onClick={props.onClearGuides}
      >
        {unit}
        {/* Density = Touch: the corner is a 44 px button, and with guides placed it shows that it clears them. */}
        {props.guides > 0 && (
          <span className="et-ruler-clear" aria-hidden="true">
            <Icon name="close" size={12} />
          </span>
        )}
      </button>
    </>
  );
}
