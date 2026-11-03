// Templates with fill-in fields, example projects and their UI model (crates/commands/src/builtin/template.rs and
// examples.rs, ARCHITECTURE.md "Templates and examples"): the fields as the engine stores them in the document
// (`extras["openetch.template"]`) and lists them (`template.fields`), the fill-in form's draft and the values it sends
// (`template.fill`), new fields for the designer's dialog, and `example.list`. Pure; tested in templateModel.test.ts.

import type { Invocation } from "../api/client.ts";
import type { CutSettings, FontKind, GDocument } from "../api/types.ts";

export const TPL = {
  fields: "template.fields",
  setField: "template.set_field",
  removeField: "template.remove_field",
  setLock: "template.set_lock",
  fill: "template.fill",
  examples: "example.list",
  newDoc: "doc.new",
} as const;

export const TEMPLATE_KEY = "openetch.template";

export type FieldKind = "text" | "date" | "font" | "preset" | "serial";
export const FIELD_KINDS: FieldKind[] = ["text", "date", "font", "preset", "serial"];

export interface Choice {
  label: string;
  font?: { family: string; kind: FontKind };
  settings?: CutSettings;
  sub_layers?: CutSettings[];
  swatch?: string;
}

export interface TemplateField {
  id: string;
  label: string;
  kind: FieldKind;
  targets: string[];
  required: boolean;
  max_chars: number;
  multiline: boolean;
  format?: string;
  choices: Choice[];
  hint?: string;
  value?: unknown;
  /** From template.fields: the value now (a choice's label where one matches). */
  current?: unknown;
  /** From template.fields: what is wrong (a deleted target). */
  problems?: string[];
}

export interface TemplateSpec {
  fields: TemplateField[];
  lock_others: boolean;
}

function field(raw: unknown): TemplateField | null {
  if (!raw || typeof raw !== "object") return null;
  const x = raw as Record<string, unknown>;
  if (typeof x.id !== "string" || typeof x.label !== "string" || !FIELD_KINDS.includes(x.kind as FieldKind)) return null;
  const strings = (v: unknown) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);
  const choices = Array.isArray(x.choices)
    ? x.choices.flatMap((c) => (c && typeof c === "object" && typeof (c as Choice).label === "string" ? [c as Choice] : []))
    : [];
  return {
    id: x.id,
    label: x.label,
    kind: x.kind as FieldKind,
    targets: strings(x.targets),
    required: x.required !== false,
    max_chars: typeof x.max_chars === "number" ? x.max_chars : 0,
    multiline: x.multiline === true,
    format: typeof x.format === "string" ? x.format : undefined,
    choices,
    hint: typeof x.hint === "string" ? x.hint : undefined,
    value: x.value,
    current: x.current,
    problems: strings(x.problems),
  };
}

/** A spec from `template.fields`' result or the document's stored JSON. */
export function parseSpec(raw: unknown): TemplateSpec {
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const fields = Array.isArray(o.fields) ? o.fields.map(field).filter((f): f is TemplateField => !!f) : [];
  return { fields, lock_others: o.lock_others === true };
}

/** The document's template fields (from its extras; no engine call). */
export function specOf(doc: GDocument | null | undefined): TemplateSpec {
  const s = doc?.extras?.[TEMPLATE_KEY];
  if (!s) return { fields: [], lock_others: false };
  try {
    return parseSpec(JSON.parse(s));
  } catch {
    return { fields: [], lock_others: false };
  }
}

export const hasFields = (doc: GDocument | null | undefined): boolean => specOf(doc).fields.length > 0;

// ---- the fill-in form -----------------------------------------------------------------------------------------

/** What the form holds per field: text as typed, a date as yyyy-mm-dd, a choice label or a family, a number. */
export type Draft = Record<string, string>;

/** Today in the browser as yyyy-mm-dd (for the date field's Today button and an empty date). */
export function today(now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

function currentText(f: TemplateField): string {
  const v = f.current ?? f.value;
  if (typeof v === "string") return v;
  if (typeof v === "number") return String(v);
  if (v && typeof v === "object" && typeof (v as { family?: unknown }).family === "string") {
    const f = v as { family: string; kind?: FontKind };
    return fontDraft({ family: f.family, kind: f.kind ?? "system" });
  }
  return "";
}

/** The form's starting values: each field's current value (a date with none starts today). */
export function draftOf(spec: TemplateSpec, now = new Date()): Draft {
  const d: Draft = {};
  for (const f of spec.fields) {
    const cur = currentText(f);
    d[f.id] = f.kind === "date" && !/^\d{4}-\d{2}-\d{2}$/.test(cur) ? today(now) : cur;
  }
  return d;
}

/** Why the form cannot be sent (per field id), as message keys with values. */
export function draftProblems(spec: TemplateSpec, d: Draft): Record<string, { key: "tpl.required" | "tpl.tooLong" | "tpl.oneLine" | "tpl.badDate" | "tpl.badNumber" | "tpl.pick"; n?: number }> {
  const out: ReturnType<typeof draftProblems> = {};
  for (const f of spec.fields) {
    const v = d[f.id] ?? "";
    if (f.kind === "text") {
      if (f.required && !v.trim()) out[f.id] = { key: "tpl.required" };
      else if (!f.multiline && /\n/.test(v)) out[f.id] = { key: "tpl.oneLine" };
      else if (f.max_chars > 0 && [...v].length > f.max_chars) out[f.id] = { key: "tpl.tooLong", n: f.max_chars };
    } else if (f.kind === "date" && !/^\d{4}-\d{2}-\d{2}$/.test(v)) out[f.id] = { key: "tpl.badDate" };
    else if (f.kind === "serial" && !/^-?\d+$/.test(v.trim())) out[f.id] = { key: "tpl.badNumber" };
    else if ((f.kind === "preset" || (f.kind === "font" && f.choices.length > 0)) && !f.choices.some((c) => c.label === v)) out[f.id] = { key: "tpl.pick" };
  }
  return out;
}

/** `template.fill` for the fields whose value differs from `before` (null when nothing changed). */
export function fillInvocation(spec: TemplateSpec, d: Draft, before: Draft): Invocation | null {
  const values: Record<string, unknown> = {};
  for (const f of spec.fields) {
    const v = d[f.id] ?? "";
    if (v === (before[f.id] ?? "")) continue;
    if (f.kind === "serial") values[f.id] = Number.parseInt(v.trim(), 10);
    else if (f.kind === "font" && f.choices.length === 0) {
      const [kind, ...rest] = v.split("\u0000");
      values[f.id] = rest.length ? { family: rest.join("\u0000"), kind } : v;
    } else values[f.id] = f.kind === "text" ? v.replace(/\r\n?/g, "\n") : v;
  }
  return Object.keys(values).length ? { id: TPL.fill, params: { values } } : null;
}

/** A font without choices travels in the draft as `kind\0family`. */
export const fontDraft = (f: { family: string; kind: FontKind }): string => `${f.kind}\u0000${f.family}`;
export function draftFont(v: string): { family: string; kind: FontKind } | null {
  const i = v.indexOf("\u0000");
  if (i < 0) return v ? { family: v, kind: "system" } : null;
  return { family: v.slice(i + 1), kind: v.slice(0, i) as FontKind };
}

/** The `overflow` and `warnings` of a fill's result, as lines to show. */
export function fillReport(r: unknown): { overflow: { id: string; message: string }[]; warnings: string[] } {
  const o = r && typeof r === "object" ? (r as Record<string, unknown>) : {};
  const overflow = Array.isArray(o.overflow)
    ? o.overflow.flatMap((x) => (x && typeof x === "object" && typeof (x as { message?: unknown }).message === "string" ? [{ id: String((x as { id?: unknown }).id ?? ""), message: (x as { message: string }).message }] : []))
    : [];
  const warnings = Array.isArray(o.warnings) ? o.warnings.filter((w): w is string => typeof w === "string") : [];
  return { overflow, warnings };
}

// ---- defining fields ------------------------------------------------------------------------------------------

/** A field id from its label: lower-case letters, digits and '-', unique among `taken`. */
export function fieldIdFor(label: string, taken: readonly string[]): string {
  const base =
    label
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32) || "field";
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

/** `template.set_field` for `f` (only what the engine takes). */
export function setFieldInvocation(f: TemplateField, index?: number): Invocation {
  const field: Record<string, unknown> = { id: f.id, label: f.label, kind: f.kind, targets: f.targets };
  if (f.kind === "text") Object.assign(field, { required: f.required, max_chars: f.max_chars, multiline: f.multiline });
  if (f.kind === "date") field.format = f.format || "yyyy-MM-dd";
  if (f.choices.length) field.choices = f.choices;
  if (f.hint?.trim()) field.hint = f.hint.trim();
  return { id: TPL.setField, params: index === undefined ? { field } : { field, index } };
}

export const removeFieldInvocation = (id: string): Invocation => ({ id: TPL.removeField, params: { id } });
export const setLockInvocation = (on: boolean): Invocation => ({ id: TPL.setLock, params: { lock_others: on } });

/** A finish choice from a layer's settings as they are now. */
export function choiceFromLayer(label: string, layer: { settings: CutSettings; sub_layers: CutSettings[]; color: string }, swatch?: string): Choice {
  const c: Choice = { label: label.trim(), settings: layer.settings, swatch: swatch || layer.color };
  if (layer.sub_layers.length) c.sub_layers = layer.sub_layers;
  return c;
}

/** A new, empty field of `kind` aimed at `targets`. */
export function newField(kind: FieldKind, label: string, targets: string[], taken: readonly string[]): TemplateField {
  return {
    id: fieldIdFor(label, taken),
    label,
    kind,
    targets: kind === "serial" ? [] : targets,
    required: true,
    max_chars: 0,
    multiline: false,
    format: kind === "date" ? "yyyy-MM-dd" : undefined,
    choices: [],
  };
}

// ---- examples -------------------------------------------------------------------------------------------------

export interface ExampleInfo {
  id: string;
  title: string;
  summary: string;
  tags: string[];
  steps: string[];
}

export function parseExamples(r: unknown): ExampleInfo[] {
  if (!Array.isArray(r)) return [];
  return r.flatMap((e) => {
    if (!e || typeof e !== "object") return [];
    const x = e as Record<string, unknown>;
    if (typeof x.id !== "string" || typeof x.title !== "string") return [];
    const strings = (v: unknown) => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : []);
    return [{ id: x.id, title: x.title, summary: typeof x.summary === "string" ? x.summary : "", tags: strings(x.tags), steps: strings(x.steps) }];
  });
}
