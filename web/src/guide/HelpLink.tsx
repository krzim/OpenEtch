// The "?" button that opens the user guide at a page and anchor: `<HelpLink to="layers#cut-settings" topic="Cut
// settings" />`. Its accessible name is "Help: <topic>"; on touch it is a 44 px target, on a desktop a compact circle
// that fits a panel caption or a dialog title. `to` must be a string literal naming a page of docs/guide and the id of
// one of its headings, as GitHub makes it ("## Cut settings" -> "cut-settings"): helpLinks.test.ts reads every
// HelpLink in src and fails when a target is missing.

import { useT } from "../workspace/usePrefs.ts";
import { openGuide } from "./route.ts";
import "./guide-shell.css";

export interface HelpLinkProps {
  /** "page" or "page#anchor" (docs/guide/<page>.md, a heading's GitHub id). */
  to: string;
  /** What the guide explains there, for the accessible name ("Help: Cut settings"); already translated. */
  topic: string;
  className?: string;
}

export function HelpLink(p: HelpLinkProps) {
  const t = useT();
  const label = t("guide.helpFor", { topic: p.topic });
  return (
    <button
      type="button"
      className={`help-link${p.className ? ` ${p.className}` : ""}`}
      aria-label={label}
      title={label}
      data-help-target={p.to}
      onClick={(e) => {
        // Inside a menu or a row: the click is the help button's alone.
        e.stopPropagation();
        openGuide(p.to);
      }}
    >
      <span aria-hidden="true">?</span>
    </button>
  );
}
