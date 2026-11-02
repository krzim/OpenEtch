// STAGE A STUB (owner: P5). The machine status chip left of STOP in every layout (docs/ux/ui-plan-2026-11.md P5):
// Idle, Framing (pulse plus inline Stop frame), Running 42% · 3:12 (plus Pause), Paused, Offline; a tap opens the Laser
// panel or the Job sheet. The signature below is final; until P5 lands it renders nothing.

import "./laser.css";

export interface StatusChipProps {
  /** Phone: just the status dot. */
  compact?: boolean;
}

export function StatusChip(_p: StatusChipProps) {
  return null;
}
