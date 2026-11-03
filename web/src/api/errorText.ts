// The one formatter for failures shown to the user (toasts, dialog error lines, the preview card): the engine's text
// without the HTTP prefix ("POST /api/commands: 422 "), without the internal command ids that start or wrap a
// command's error ("invalid input for 'path.dogbone': ", "path.dogbone: ", "(try path.close or path.auto_join)"), and
// with object and layer ids replaced by their names. Pure, so `npm test` covers it.

import type { GDocument } from "./types.ts";

/** The command namespaces of crates/commands (`path.dogbone`, `object.lock`, ...). */
const NS = "(?:path|shape|object|layer|doc|image|node|var|text|page|tabs|group|generate|array|select|guide|stock|job|font|fit|hatch|art|logo|region|data|timing|mirror_line)";
const CMD = String.raw`${NS}(?:\.[a-z0-9_]+)+`;
const HTTP_PREFIX = /^[A-Z]+ \/api\/\S+: \d{3} /;
const INVALID_FOR = new RegExp(`^invalid input for '${CMD}': `);
const LEADING_CMD = new RegExp(`^${CMD}: `);
/** A parenthesised hint for agents that names a command ("(object.lock with locked: false)"). */
const CMD_HINT = new RegExp(String.raw`\s*\([^()]*\b${CMD}\b[^()]*\)`, "g");

/** Replace internal object and layer ids in an engine message with their names ("obj-5" -> "Ellipse [0,2]"). */
export function humanizeIds(msg: string, doc: GDocument | null | undefined): string {
  if (!doc) return msg;
  const names = new Map<string, string>();
  for (const o of doc.objects) names.set(o.id, `"${o.name || o.id}"`);
  for (const l of doc.layers) names.set(l.id, `layer "${l.name}"`);
  return msg.replace(/\b(obj|layer)-\d+\b/g, (id) => names.get(id) ?? id);
}

/** An engine or browser message for display (see the file comment). */
export function userMessage(msg: string, doc?: GDocument | null): string {
  let m = msg.trim().replace(HTTP_PREFIX, "");
  m = m.replace(INVALID_FOR, "").replace(LEADING_CMD, "").replace(LEADING_CMD, "");
  m = m.replace(CMD_HINT, "").trim();
  if (!m) return msg;
  // A sentence starts with a capital, unless it starts with a parameter name ("diameter_mm must be > 0").
  if (/^[a-z]+(?=[\s,;:.]|$)/.test(m)) m = m.charAt(0).toUpperCase() + m.slice(1);
  return humanizeIds(m, doc);
}

/** The display text of a thrown value: the engine's own `detail` when an ApiError has one, cleaned by `userMessage`. */
export function errorText(e: unknown, doc?: GDocument | null): string {
  const detail = e && typeof e === "object" && "detail" in e ? (e as { detail?: unknown }).detail : undefined;
  const raw = typeof detail === "string" && detail ? detail : e instanceof Error ? e.message : String(e);
  return userMessage(raw, doc);
}
