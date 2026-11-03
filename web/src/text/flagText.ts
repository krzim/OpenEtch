// A contact-sheet problem in the language chosen in Preferences: the engine sends each one as a `code` with its
// `params` (crates/commands/src/builtin/contact_sheet.rs) plus its English `message`, which stays the fallback for a
// code this app does not know (an older app with a newer engine). Used by the contact sheet and its pre-flight check.

import { translateDynamic, type Vars } from "../workspace/i18n.ts";
import { getPrefs } from "../workspace/prefs.ts";

export interface CodedProblem {
  code?: string;
  params?: Record<string, unknown>;
  message?: string;
}

function vars(params: Record<string, unknown> | undefined): Vars {
  const out: Vars = {};
  for (const [k, v] of Object.entries(params ?? {})) {
    if (typeof v === "number" || typeof v === "string") out[k] = v;
  }
  return out;
}

/** The problem's text, localised by its code (the engine's English message for an unknown code). */
export function problemText(p: CodedProblem, lang: string = getPrefs().language): string {
  const message = p.message ?? "";
  if (!p.code) return message;
  return translateDynamic(lang, `sheet.code.${p.code}`, message, vars(p.params));
}
