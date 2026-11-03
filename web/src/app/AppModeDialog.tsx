// The "Use as an app" dialog (docs/ux/ui-plan-2026-11.md 1.3, P4): detects the platform and the display mode and shows
// the steps that give OpenEtch its own window there (Safari Add to Dock, Add to Home Screen, Chrome/Edge Open as
// window or Install). Opened from Help, More (phone), command search (`help.useAsApp`), the Camera's HTTPS note, and
// by itself once on the first touch visit (`offerAppModeOnce`, called by ConnectGate).
//
// It renders into a root of its own (created on first use), so it needs no host component in the app.

import { useEffect, useState, useSyncExternalStore } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Dialog } from "../components/Dialog.tsx";
import { isCockpitHash } from "../production/model.ts";
import { displayModeNow, isAppWindow, useDisplayMode, useFullscreen } from "../workspace/fullscreen.ts";
import { hintFor } from "../workspace/hotkeys.ts";
import { getPrefs } from "../workspace/prefs.ts";
import { useT } from "../workspace/usePrefs.ts";
import { APP_MODE_OFFERED_KEY, installGuide, openWhenClear, shouldOfferAppMode } from "./appMode.ts";
import "./app.css";

/** Chromium's install prompt, kept from `beforeinstallprompt` (only on a secure origin). */
interface InstallPromptEvent extends Event {
  prompt: () => Promise<void>;
  userChoice?: Promise<{ outcome: "accepted" | "dismissed" }>;
}

let open = false;
/** Set while the first-visit offer (not Help > Use as an app) is open: puts it back in the queue. */
let requeueOffer: (() => void) | null = null;
let installPrompt: InstallPromptEvent | null = null;
const listeners = new Set<() => void>();
let root: Root | null = null;

function emit(): void {
  for (const fn of [...listeners]) fn();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => void listeners.delete(fn);
}

if (typeof window !== "undefined") {
  // Keep Chromium's install prompt for the dialog's Install button (it fires once, early, on a secure origin).
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    installPrompt = e as InstallPromptEvent;
    emit();
  });
  window.addEventListener("appinstalled", () => {
    installPrompt = null;
    emit();
  });
}

function ensureRoot(): void {
  if (root || typeof document === "undefined") return;
  const host = document.createElement("div");
  host.id = "oe-app-mode-host";
  document.body.appendChild(host);
  root = createRoot(host);
  root.render(<AppModeHost />);
}

/** Open the "Use as an app" dialog (no-op when the app already runs as an app window). */
export function openAppModeDialog(): void {
  if (isAppWindow(displayModeNow())) return;
  ensureRoot();
  open = true;
  emit();
}

/** Whether it is open (it renders in its own root, so the app's key handlers ask here and hold their keys). */
export function appModeDialogOpen(): boolean {
  return open;
}

/** Close it. */
export function closeAppModeDialog(): void {
  if (!open) return;
  open = false;
  requeueOffer = null;
  emit();
}

/** The production cockpit opened: an open first-visit offer steps aside and comes back once the cockpit is gone. */
export function postponeAppModeOffer(): void {
  const again = requeueOffer;
  if (!open || !again) return;
  closeAppModeDialog();
  again();
}

/** Whether Help > "Use as an app" should be listed (not inside an app window). */
export function useAppModeOffered(): boolean {
  return !isAppWindow(useDisplayMode());
}

/** On the first touch visit in a browser tab, open the dialog once (a moment after the app has come up). */
export function offerAppModeOnce(): void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
  let offered = false;
  try {
    offered = localStorage.getItem(APP_MODE_OFFERED_KEY) === "1";
  } catch {
    return; // No storage: it would come back on every visit.
  }
  if (!shouldOfferAppMode(window.matchMedia("(pointer: coarse)").matches, displayModeNow(), offered)) return;
  try {
    localStorage.setItem(APP_MODE_OFFERED_KEY, "1");
  } catch {
    return;
  }
  // Not over another dialog, nor over the production cockpit (an operator's full screen, often a tablet's first visit).
  const busy = () => !!document.querySelector('[aria-modal="true"]') || isCockpitHash(location.hash);
  // The cockpit may still open while the offer is up (it can open just as the timer fires): postponeAppModeOffer.
  const offer = () => {
    openAppModeDialog();
    if (open) requeueOffer = queue;
  };
  const queue = () => openWhenClear(busy, offer, (f) => window.setTimeout(f, 2000));
  window.setTimeout(queue, 1500);
}


function AppModeHost() {
  const isOpen = useSyncExternalStore(subscribe, () => open);
  return isOpen ? <AppModeDialog onClose={closeAppModeDialog} /> : null;
}

function AppModeDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const mode = useDisplayMode();
  const fs = useFullscreen();
  const prompt = useSyncExternalStore(subscribe, () => installPrompt);
  const [installing, setInstalling] = useState(false);
  const guide = installGuide(navigator.userAgent, mode, window.isSecureContext, navigator.maxTouchPoints ?? 0);
  // Became an app window meanwhile (installed from the browser's own menu): nothing left to explain.
  useEffect(() => {
    if (guide.hidden) onClose();
  }, [guide.hidden, onClose]);
  const fsKey = hintFor("view.fullscreen", getPrefs().hotkeys);
  const modeText = mode === "fullscreen" ? t("app.mode.mode.fullscreen") : guide.hidden ? t("app.mode.mode.app") : t("app.mode.mode.browser");
  const install = async () => {
    if (!prompt) return;
    setInstalling(true);
    try {
      await prompt.prompt();
      await prompt.userChoice;
    } finally {
      installPrompt = null;
      setInstalling(false);
      emit();
    }
  };
  return (
    <Dialog
      help="getting-started#install-openetch-as-an-app"
      title={t("app.mode.title")}
      onClose={onClose}
      size="md"
      footer={
        <>
          {guide.installPrompt && prompt && (
            <button type="button" className="btn btn-primary" disabled={installing} onClick={() => void install()}>
              {t("app.mode.install")}
            </button>
          )}
          <button type="button" className="btn" onClick={onClose} autoFocus>
            {t("app.mode.close")}
          </button>
        </>
      }
    >
      <div className="app-mode">
        <p>{t("app.mode.lead")}</p>
        <p className="app-mode-where">
          <strong>{t(`app.mode.platform.${guide.platform}` as const)}</strong> · {t("app.mode.current", { mode: modeText })}
        </p>
        <ol className="app-mode-steps">
          {guide.steps.map((k) => (
            <li key={k}>{t(k)}</li>
          ))}
        </ol>
        {guide.fullscreenHint && fs.supported && fsKey && <p className="app-mode-note">{t("app.mode.orFullscreen", { key: fsKey })}</p>}
        {guide.httpNote && (
          <p className="app-mode-note app-mode-http">
            {t("app.mode.httpNote", { origin: window.location.origin })} {t("app.mode.httpsNote")}
          </p>
        )}
      </div>
    </Dialog>
  );
}
