// The nudge pad (docs/ux/ui-plan-2026-11.md 1.2, audit T9): arrow nudges and 90° turns for the selection without a
// keyboard, in the Properties panel's position section on coarse pointers. Each button runs the same key as the
// keyboard (touch/actions.ts), so the step sizes are Preferences > Nudge's: Normal (none), Large (Shift), Small (Ctrl),
// Tiny (Ctrl+Shift). Holding a button repeats it.

import { useEffect, useRef, useState } from "react";
import { useT } from "../workspace/usePrefs.ts";
import { formatLength } from "../workspace/units.ts";
import type { NudgeSteps } from "../editing.ts";
import type { LengthUnit } from "../workspace/prefs.ts";
import { runTouchAction } from "./actions.ts";
import "./touch.css";
import { Icon } from "../components/ui.tsx";

type Step = "normal" | "shift" | "ctrl" | "ctrlShift";
const STEP_MODS: Record<Step, { shift?: boolean; ctrl?: boolean }> = {
  normal: {},
  shift: { shift: true },
  ctrl: { ctrl: true },
  ctrlShift: { shift: true, ctrl: true },
};

/** Run `fn` now, then again every 90 ms after a 400 ms hold, until the pointer lifts or leaves. */
function useHoldRepeat() {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stop = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };
  useEffect(() => stop, []);
  return (fn: () => void) => ({
    onPointerDown: (e: React.PointerEvent<HTMLButtonElement>) => {
      e.preventDefault();
      stop();
      fn();
      const again = () => {
        fn();
        timer.current = setTimeout(again, 90);
      };
      timer.current = setTimeout(again, 400);
    },
    onPointerUp: stop,
    onPointerLeave: stop,
    onPointerCancel: stop,
    // Keyboard users: Enter / Space click once.
    onKeyDown: (e: React.KeyboardEvent<HTMLButtonElement>) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        fn();
      }
    },
  });
}

export function NudgePad({ steps, unit }: { steps: NudgeSteps; unit: LengthUnit }) {
  const t = useT();
  const [step, setStep] = useState<Step>("normal");
  const hold = useHoldRepeat();
  const run = (id: string) => () => void runTouchAction(id, STEP_MODS[step]);
  const size: Record<Step, number> = { normal: steps.normal, shift: steps.shift, ctrl: steps.ctrl, ctrlShift: steps.ctrlShift };
  const btn = (id: string, icon: string, label: string) => (
    <button type="button" className="nudge-btn" title={label} aria-label={label} {...hold(run(id))}>
      <Icon name={icon} size={22} />
    </button>
  );
  return (
    <div className="nudge-pad" role="group" aria-label={t("touch.nudge")}>
      <div className="nudge-grid">
        <span />
        {btn("nudgeUp", "arrowUp", t("touch.nudgeUp"))}
        <span />
        {btn("nudgeLeft", "arrowLeft", t("touch.nudgeLeft"))}
        <button type="button" className="nudge-btn" title={t("touch.rotate90")} aria-label={t("touch.rotate90")} onClick={() => void runTouchAction("rotate90")}>
          <Icon name="rotate" size={22} />
        </button>
        {btn("nudgeRight", "arrowRight", t("touch.nudgeRight"))}
        <span />
        {btn("nudgeDown", "arrowDown", t("touch.nudgeDown"))}
        <span />
      </div>
      <label className="nudge-step">
        <span>{t("touch.nudgeStep")}</span>
        <select value={step} onChange={(e) => setStep(e.target.value as Step)}>
          <option value="normal">
            {t("touch.stepNormal")} · {formatLength(size.normal, unit, 3)}
          </option>
          <option value="shift">
            {t("touch.stepLarge")} · {formatLength(size.shift, unit, 3)}
          </option>
          <option value="ctrl">
            {t("touch.stepSmall")} · {formatLength(size.ctrl, unit, 3)}
          </option>
          <option value="ctrlShift">
            {t("touch.stepTiny")} · {formatLength(size.ctrlShift, unit, 3)}
          </option>
        </select>
      </label>
    </div>
  );
}
