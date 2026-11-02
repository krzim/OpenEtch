// The top-bar Full screen toggle (docs/ux/ui-plan-2026-11.md 1.3, P4), for P1 to place in the top bar's actions.
// Hidden where full screen is unsupported (iPhone, iOS Home Screen apps). Its state follows F11 too; then it says to
// press F11. Under it, the one-time tip after Esc left full screen (or that F11 owns it).
// Its icons are the registry's fullscreen / fullscreenExit (components/ui.tsx).

import { useEffect } from "react";
import { dismissFullscreenTip, useFullscreen, useFullscreenTip } from "../workspace/fullscreen.ts";
import { hintFor } from "../workspace/hotkeys.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import "./app.css";
import { Icon } from "../components/ui.tsx";


/** How long the tip stays unless dismissed. */
const TIP_MS = 9000;

export function FullscreenButton({ className = "icon-btn" }: { className?: string }) {
  const fs = useFullscreen();
  const tip = useFullscreenTip();
  const t = useT();
  const key = hintFor("view.fullscreen", usePrefs().hotkeys);
  useEffect(() => {
    if (!tip) return;
    const id = window.setTimeout(dismissFullscreenTip, TIP_MS);
    return () => window.clearTimeout(id);
  }, [tip]);
  if (!fs.supported) return null;
  const label = fs.browserOwned ? t("app.fs.exitF11") : fs.active ? t("app.fs.exit") : t("app.fs.enter");
  return (
    <span className="fs-toggle">
      <button
        type="button"
        className={className}
        onClick={fs.toggle}
        aria-pressed={fs.active}
        aria-label={label}
        title={key && !fs.browserOwned ? `${label} (${key})` : label}
      >
        <Icon name={fs.active ? "fullscreenExit" : "fullscreen"} size={20} />
      </button>
      {tip && (
        <span className="fs-tip" role="status">
          <span>{tip === "browser" ? t("app.fs.tipBrowser") : tip === "refused" ? t("app.fs.tipRefused") : t("app.fs.tipEscape", { key: key || "Alt+Enter" })}</span>
          <button type="button" className="btn fs-tip-ok" onClick={dismissFullscreenTip}>
            {t("app.fs.tipOk")}
          </button>
        </span>
      )}
    </span>
  );
}
