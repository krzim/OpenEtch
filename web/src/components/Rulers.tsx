// Rulers along the top and left of the canvas, in field millimetres (+Y up, so the left ruler counts up), with the
// guides marked on them. Dragging out of a ruler makes a guide (top ruler: horizontal, left ruler: vertical, as in
// LightBurn); dropping it back on a ruler throws it away. The corner square clears all guides.

import type { RenderCtx } from "../canvasTool.ts";
import type { Vec2 } from "../api/types.ts";
import { rulerTicks } from "../drawTools.ts";

/** Ruler thickness, px. */
export const RULER_PX = 20;

export function Rulers(props: {
  r: RenderCtx;
  guideDrag: { axis: "x" | "y"; v: number; remove: boolean } | null;
  onDrag: (axis: "x" | "y", screen: Vec2, phase: "move" | "end" | "cancel") => void;
  guides: number;
  onClearGuides: () => void;
}) {
  const { r } = props;
  const [w, h] = r.size;
  const xs = rulerTicks(r.toField([RULER_PX, 0])[0], r.toField([w, 0])[0], r.scale);
  const ys = rulerTicks(r.toField([0, h])[1], r.toField([0, RULER_PX])[1], r.scale);

  /** Pointer handlers for one ruler: the guide follows the pointer until release. */
  const dragHandlers = (axis: "x" | "y") => {
    const local = (e: React.PointerEvent<HTMLDivElement>): Vec2 => {
      const vp = e.currentTarget.closest(".viewport")?.getBoundingClientRect();
      return vp ? [e.clientX - vp.left, e.clientY - vp.top] : [e.clientX, e.clientY];
    };
    return {
      onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => {
        if (e.pointerType === "mouse" && e.button !== 0) return;
        e.preventDefault();
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          // Synthetic events cannot capture; the drag still works while the pointer stays over the ruler.
        }
        props.onDrag(axis, local(e), "move");
      },
      onPointerMove: (e: React.PointerEvent<HTMLDivElement>) => {
        if (!e.currentTarget.hasPointerCapture?.(e.pointerId)) return;
        props.onDrag(axis, local(e), "move");
      },
      onPointerUp: (e: React.PointerEvent<HTMLDivElement>) => {
        if (!e.currentTarget.hasPointerCapture?.(e.pointerId)) return;
        props.onDrag(axis, local(e), "end");
      },
      onPointerCancel: (e: React.PointerEvent<HTMLDivElement>) => props.onDrag(axis, local(e), "cancel"),
    };
  };

  // The top ruler makes horizontal guides (y = const) and measures x; the left one the other way round.
  const gd = props.guideDrag;
  return (
    <>
      <div className="et-ruler et-ruler-top" title="Drag down to place a horizontal guide" {...dragHandlers("y")}>
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
      <div className="et-ruler et-ruler-left" title="Drag right to place a vertical guide" {...dragHandlers("x")}>
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
        title={props.guides ? `Clear ${props.guides} guide${props.guides === 1 ? "" : "s"}` : "Rulers in mm. Drag out of a ruler to place a guide."}
        aria-label="Clear guides"
        disabled={!props.guides}
        onClick={props.onClearGuides}
      >
        mm
      </button>
    </>
  );
}
