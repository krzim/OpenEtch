// STAGE A STUB (owner: P4). The "Use as an app" dialog (docs/ux/ui-plan-2026-11.md 1.3): detects the platform and the
// display mode and shows the right steps (Safari Add to Dock, Add to Home Screen, Chrome/Edge Open as window). Opened
// from Help, More (phone), command search (`help.useAsApp`) and once on the first touch visit. The signature below is
// final; until P4 lands it does nothing.

import "./app.css";

/** Open the "Use as an app" dialog (no-op when the app already runs standalone). */
export function openAppModeDialog(): void {}
