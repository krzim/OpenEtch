// The top-bar Full screen toggle (docs/ux/ui-plan-2026-11.md 1.3, P4), for P1 to place in the top bar's actions.
// Hidden where full screen is unsupported (iPhone, iOS Home Screen apps). Its state follows F11 too; then it says to
// press F11. Under it, the one-time tip after Esc left full screen (or that F11 owns it).
//
// Icon path data: Lucide "maximize" and "minimize" (ISC License, Copyright (c) Lucide Contributors 2022, see
// web/src/icons/THIRD_PARTY_NOTICES.txt), until P2's icon registry has them.

import { useEffect } from "react";
import { dismissFullscreenTip, useFullscreen, useFullscreenTip } from "../workspace/fullscreen.ts";
import { hintFor } from "../workspace/hotkeys.ts";
import { usePrefs, useT } from "../workspace/usePrefs.ts";
import "./app.css";

const MAXIMIZE = ["M8 3H5a2 2 0 0 0-2 2v3", "M21 8V5a2 2 0 0 0-2-2h-3", "M3 16v3a2 2 0 0 0 2 2h3", "M16 21h3a2 2 0 0 0 2-2v-3"];
const MINIMIZE = ["M8 3v3a2 2 0 0 1-2 2H3", "M21 8h-3a2 2 0 0 1-2-2V3", "M3 16h3a2 2 0 0 1 2 2v3", "M16 21v-3a2 2 0 0 1 2-2h3"];

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
        <svg
          className="icon"
          viewBox="0 0 24 24"
          width={20}
          height={20}
          style={{ width: "calc(20px * var(--icon-scale, 1))", height: "calc(20px * var(--icon-scale, 1))" }}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          {(fs.active ? MINIMIZE : MAXIMIZE).map((d) => (
            <path key={d} d={d} />
          ))}
        </svg>
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
