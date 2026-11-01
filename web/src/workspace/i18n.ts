// Minimal i18n: flat message tables per language, `{name}` placeholders, English fallback. English (locales/en.ts)
// is the source of truth for the keys, so a typo in a key is a type error. Another language is one more table
// registered in LOCALES with the same keys (missing ones fall back to English); the language selector in
// Preferences lists whatever is registered.

import { EN, type MsgKey } from "./locales/en.ts";
import { ES } from "./locales/es.ts";

export type { MsgKey };
export type Messages = Partial<Record<MsgKey, string>>;
export type Vars = Record<string, string | number>;

interface Locale {
  /** Name of the language in itself, for the selector. */
  name: string;
  messages: Messages;
}

const LOCALES: Record<string, Locale> = {
  en: { name: "English", messages: EN },
  // A machine-translated skeleton (locales/es.ts) that proves the framework; other messages fall back to English.
  es: { name: "Español (traducción automática)", messages: ES },
};

/** Add (or replace) a language at runtime. */
export function registerLocale(code: string, name: string, messages: Messages): void {
  LOCALES[code] = { name, messages };
}

/** Registered languages, English first. */
export function languages(): { code: string; name: string }[] {
  return Object.entries(LOCALES)
    .map(([code, l]) => ({ code, name: l.name }))
    .sort((a, b) => (a.code === "en" ? -1 : b.code === "en" ? 1 : a.name.localeCompare(b.name)));
}

/** Replace `{name}` with vars.name; unknown placeholders stay as they are. */
export function interpolate(s: string, vars?: Vars): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

/** The message for `key` in `lang`, else English, else the key itself. */
export function translate(lang: string, key: MsgKey, vars?: Vars): string {
  const base = lang.split("-")[0];
  const msg = LOCALES[lang]?.messages[key] ?? LOCALES[base]?.messages[key] ?? EN[key] ?? key;
  return interpolate(msg, vars);
}

/** Like `translate` for keys built at runtime (hotkey action labels): `fallback` when the key is unknown. */
export function translateDynamic(lang: string, key: string, fallback: string, vars?: Vars): string {
  if (key in EN) return translate(lang, key as MsgKey, vars);
  return interpolate(fallback, vars);
}

export type T = (key: MsgKey, vars?: Vars) => string;
