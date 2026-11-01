// Translation outside React (pure modules: api/*.ts messages): the message in the language chosen in Preferences now.
// Components use `useT()` (usePrefs.ts), which also re-renders when the language changes.

import { translate, type MsgKey, type Vars } from "./i18n.ts";
import { getPrefs } from "./prefs.ts";

export function tr(key: MsgKey, vars?: Vars): string {
  return translate(getPrefs().language, key, vars);
}
