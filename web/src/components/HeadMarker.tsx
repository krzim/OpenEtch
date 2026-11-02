// Show Last Position (LightBurn's Laser window button) for a GRBL gantry: a crosshair on the canvas at the head's last
// known position (api/grblPositions.ts). Drawn inside the Viewport's view transform, in field mm; `px` turns screen
// pixels into mm so the mark keeps its size at every zoom. Only a drawing: it takes no pointer events.

import { useSyncExternalStore } from "react";
import { headState, subscribeHead } from "../api/grblPositions.ts";
import { useT } from "../workspace/usePrefs.ts";
import "./headMarker.css";

export function HeadMarker(p: { px: (screenPx: number) => number }) {
  const t = useT();
  const h = useSyncExternalStore(subscribeHead, headState);
  if (!h.show || !h.field) return null;
  const [x, y] = h.field;
  const r = p.px(7);
  const arm = p.px(14);
  return (
    <g className="head-marker" pointerEvents="none" data-x={x} data-y={y}>
      <title>{t("move.lastPositionTitle")}</title>
      <line x1={x - arm} y1={y} x2={x + arm} y2={y} strokeWidth={p.px(1.5)} />
      <line x1={x} y1={y - arm} x2={x} y2={y + arm} strokeWidth={p.px(1.5)} />
      <circle cx={x} cy={y} r={r} strokeWidth={p.px(1.5)} />
    </g>
  );
}
