// STAGE A STUB (owner: P4). The "Connect to OpenEtch" card (docs/ux/ui-plan-2026-11.md 1.3): shown instead of a broken
// UI when the engine answers 401 or no access token is stored; pasting the token reconnects without a reload. P1 gates
// the app on `useNeedsToken()`. The signatures below are final; until P4 lands no token is ever needed and the card
// renders nothing.

import "./app.css";

export interface ConnectScreenProps {
  /** The token was accepted: the app can talk to the engine again. */
  onConnected: () => void;
}

export function ConnectScreen(_p: ConnectScreenProps) {
  return null;
}

/** The engine refused the stored token (401) or there is none: show ConnectScreen. */
export function useNeedsToken(): boolean {
  return false;
}
