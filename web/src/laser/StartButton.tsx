// STAGE A STUB (owner: P5). The Start button (docs/ux/ui-plan-2026-11.md P5): amber, smaller than STOP, its disabled
// reason as visible text, the scope and time in its label; it calls laser/startFlow.ts. P2 places the toolbar variant
// in the Laser ribbon, the Laser dock uses "dock" and the phone's Job sheet "sheet". The signature below is final;
// until P5 lands it renders nothing (keep the existing Request run buttons until then).

import "./laser.css";

export type StartButtonVariant = "toolbar" | "dock" | "sheet";

export interface StartButtonProps {
  variant: StartButtonVariant;
}

export function StartButton(_p: StartButtonProps) {
  return null;
}
