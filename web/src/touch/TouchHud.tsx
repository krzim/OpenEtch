// The touch HUD (docs/ux/ui-plan-2026-11.md 1.2): two floating bars over the canvas.
// - The modifier bar (bottom-left, coarse pointers or Density = Touch): the keys a finger lacks, as toggles that change
//   with the tool (touch/hudModel.ts). A tap latches a toggle for the next gesture, a double-tap locks it until tapped
//   again; they feed touch/virtualMods.ts, which the canvas ORs with the keyboard. Lit toggles show their state.
// - The selection action bar (a pill above the selection; Preferences > Selection bar: Auto = on coarse pointers,
//   Always, Never): Copy, Paste, Duplicate, Delete, Group / Ungroup, Rotate 90, Flip and ⋯ (the full context menu).
// P1 mounts it in `.center-stage`: `<TouchHud tool={tool} selectionCount={n} onAction={runTouchAction} />`
// (touch/actions.ts runs each id through the action's own hotkey, so App.tsx keeps the one implementation).
// Both bars are canvas HUD items (hud/CanvasHud.tsx): the modifier bar in the bottom-left slot, the action bar
// anchored above the selection, so neither covers another floating control. The action bar hides while a bottom sheet
// covers the selection (hudState coverTop). The bars never cover STOP (the HUD sits inside the canvas, below --z-stop)
// and fade during a gesture.

import { useRef, useState } from "react";
import type { Tool } from "../components/Viewport.tsx";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import { useLayoutMode } from "../workspace/layoutMode.ts";
import { useHud } from "./hudState.ts";
import { barActions, barCapacity, fitBarActions, modToggles, selectionCovered } from "./hudModel.ts";
import { HudItem, useHudAreaWidth } from "../hud/CanvasHud.tsx";
import { HUD_GAP } from "../hud/hudLayout.ts";
import { Menu, type MenuItem } from "../components/Menu.tsx";
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
  const all = showBar ? barActions(selectionCount, hud.grouped) : [];
  // As many buttons as the canvas is wide (a narrow phone has no room for all of them): the rest go under the last ⋯,
  // so the bar never scrolls sideways or shows a button cut off.
  const areaW = useHudAreaWidth();
  const { shown: actions, overflow } = fitBarActions(all, areaW > 0 ? barCapacity(areaW - 2 * HUD_GAP) : all.length);
  const [overflowAt, setOverflowAt] = useState<{ rect: DOMRect; owner: HTMLElement } | null>(null);
  const overflowItems: MenuItem[] = [
    ...overflow.map((a) => ({ label: t(a.label), icon: a.icon, danger: a.id === "delete", action: () => onAction(a.id) })),
    { sep: true as const },
    { label: t("touch.allActions"), icon: "more", action: () => onAction("menu") },
  ];

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

  // Hidden while a sheet (the phone's Layers, Job or More) covers the selection. The canvas HUD places it above the
  // selection (or below it), clear of the HUD's other controls and of the sheet.
  const at = showBar && hud.selRect && !selectionCovered(hud.selRect, hud.coverTop) ? hud.selRect : null;

  if (!toggles.length && !actions.length && !overflowAt) return null;
  return (
    <>
      {toggles.length > 0 && (
        <HudItem id="touch-mods" slot="bl" rank={30} optional>
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
        </HudItem>
      )}
      {at && actions.length > 0 && (
        <HudItem id="touch-actions" slot="bc" rank={80} minW={44 * 4 + 16} optional anchor={{ rect: at, side: "above" }}>
          <div className={`touch-actions${hud.busy ? " busy" : ""}`} role="toolbar" aria-label={t("touch.actionBar")}>
            {actions.map((a) => (
              <button
                key={a.id}
                type="button"
                className={`touch-action${a.id === "delete" ? " danger" : ""}`}
                title={t(a.label)}
                aria-label={t(a.label)}
                aria-haspopup={a.id === "menu" && overflow.length > 0 ? "menu" : undefined}
                onPointerDown={(e) => e.preventDefault()}
                onClick={(e) => {
                  if (a.id === "menu" && overflow.length > 0) {
                    const owner = e.currentTarget;
                    setOverflowAt((o) => (o ? null : { rect: owner.getBoundingClientRect(), owner }));
                  } else onAction(a.id);
                }}
              >
                <Icon name={a.icon} size={24} />
              </button>
            ))}
          </div>
        </HudItem>
      )}
      {overflowAt && at && overflow.length > 0 && (
        <Menu items={overflowItems} anchor={overflowAt.rect} placement="above" align="end" label={t("touch.more")} owner={overflowAt.owner} onClose={() => setOverflowAt(null)} />
      )}
    </>
  );
}
