// The touch HUD (docs/ux/ui-plan-2026-11.md 1.2): two floating bars over the canvas.
// - The modifier bar (bottom-left, coarse pointers or Density = Touch): the keys a finger lacks, as toggles that change
//   with the tool (touch/hudModel.ts). A tap latches a toggle for the next gesture, a double-tap locks it until tapped
//   again; they feed touch/virtualMods.ts, which the canvas ORs with the keyboard. Lit toggles show their state.
// - The selection action bar (a pill above the selection; Preferences > Selection bar: Auto = on coarse pointers,
//   Always, Never): Copy, Paste, Duplicate, Delete, Group / Ungroup, Rotate 90, Flip and ⋯ (the full context menu).
// P1 mounts it in `.center-stage`: `<TouchHud tool={tool} selectionCount={n} onAction={runTouchAction} />`
// (touch/actions.ts runs each id through the action's own hotkey, so App.tsx keeps the one implementation).
// The action bar hides while a bottom sheet covers the selection (hudState coverTop). The bars never cover STOP
// (they sit inside the canvas at --z-float, below --z-stop) and fade during a gesture.

import { useLayoutEffect, useRef, useState } from "react";
import type { Tool } from "../components/Viewport.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { useHud } from "./hudState.ts";
import { actionBarSpot, barActions, modToggles } from "./hudModel.ts";
import { latch, lock, useVirtualMods, type VirtualMod } from "./virtualMods.ts";
import "./touch.css";
import { Icon } from "../components/ui.tsx";

export interface TouchHudProps {
  /** The active canvas tool (picks the modifier toggles: Select, Rect/Ellipse, Line/Pen, Node, ...). */
  tool: Tool;
  /** Selected objects (the action bar shows with one or more). */
  selectionCount: number;
  /** An action bar button: "copy", "paste", "duplicate", "delete", "group", "ungroup", "rotate90", "flipH", "flipV",
   * "menu" (P3 owns the ids; touch/actions.ts runTouchAction runs them). */
  onAction: (id: string) => void;
}

/** Two taps on one toggle within this many ms lock it. */
const DOUBLE_TAP_MS = 350;

export function TouchHud({ tool, selectionCount, onAction }: TouchHudProps) {
  const t = useT();
  const layout = useLayoutMode();
  const showBarPref = usePrefs().showSelectionBar;
  const hud = useHud();
  const vm = useVirtualMods();
  const coarse = layout.pointer === "coarse" || layout.density === "touch";
  const toggles = coarse ? modToggles(tool, hud.rotateMode) : [];
  const showBar = tool === "select" && selectionCount > 0 && !!hud.selRect && (showBarPref === "always" || (showBarPref === "auto" && coarse));
  const actions = showBar ? barActions(selectionCount, hud.grouped) : [];

  const lastTap = useRef<{ mod: VirtualMod; t: number } | null>(null);
  const tap = (mod: VirtualMod) => {
    const now = performance.now();
    const lt = lastTap.current;
    lastTap.current = { mod, t: now };
    if (lt && lt.mod === mod && now - lt.t < DOUBLE_TAP_MS) {
      lastTap.current = null;
      // The first tap latched it: the second makes it stay.
      if (!vm.locked.includes(mod)) lock(mod);
      return;
    }
    latch(mod);
  };

  // The action bar's size, for its placement above the selection.
  const barRef = useRef<HTMLDivElement>(null);
  const [barSize, setBarSize] = useState<[number, number]>([360, 52]);
  useLayoutEffect(() => {
    const el = barRef.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    if (w !== barSize[0] || h !== barSize[1]) setBarSize([w, h]);
  });
  const canvas = hud.canvasRect;
  // Hidden while a sheet (the phone's Layers, Job or More) covers the selection; kept above the sheet otherwise.
  const at = showBar && canvas ? actionBarSpot(hud.selRect, canvas, barSize[0], barSize[1], hud.coverTop) : null;

  if (!toggles.length && !actions.length) return null;
  return (
    <>
      {toggles.length > 0 && (
        <div className={`touch-mods${hud.busy ? " busy" : ""}`} role="toolbar" aria-label={t("touch.modBar")} title={t("touch.modHint")}>
          {toggles.map((g) => {
            const locked = vm.locked.includes(g.mod);
            const on = vm[g.mod];
            const label = t(g.label);
            return (
              <button
                key={`${g.mod}-${g.label}`}
                type="button"
                className={`touch-mod${on ? " on" : ""}${locked ? " locked" : ""}`}
                aria-pressed={on}
                title={t(g.tip)}
                aria-label={locked ? t("touch.locked", { name: label }) : label}
                // Keep the keyboard focus on the canvas.
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => tap(g.mod)}
              >
                <span className="touch-mod-label">{label}</span>
                {g.sub && <span className="touch-mod-sub">{t(g.sub)}</span>}
              </button>
            );
          })}
        </div>
      )}
      {at && actions.length > 0 && (
        <div
          ref={barRef}
          className={`touch-actions${hud.busy ? " busy" : ""}`}
          role="toolbar"
          aria-label={t("touch.actionBar")}
          style={{ left: at.left, top: at.top }}
        >
          {actions.map((a) => (
            <button
              key={a.id}
              type="button"
              className={`touch-action${a.id === "delete" ? " danger" : ""}`}
              title={t(a.label)}
              aria-label={t(a.label)}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => onAction(a.id)}
            >
              <Icon name={a.icon} size={24} />
            </button>
          ))}
        </div>
      )}
    </>
  );
}
