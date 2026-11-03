// The contact sheet (short-list #10): every serial number or CSV row as a run would burn it, from `var.contact_sheet`
// (crates/commands/src/builtin/contact_sheet.rs), with its problems. Pure: the answer checked into a model the grid
// draws, the counts for the summary chips and the request for a page. Tested in contactSheet.test.ts.

import type { Invocation } from "../api/client.ts";
import { TXT } from "./textModel.ts";

export type FlagKind = "overflow" | "missing_glyph" | "empty" | "shrunk" | "error" | "warning";
export const FLAG_KINDS: FlagKind[] = ["overflow", "missing_glyph", "empty", "shrunk", "error"];

export interface Flag {
  kind: FlagKind;
  message: string;
}

export interface SheetText {
  id: string;
  shown: string;
  flags: Flag[];
  d?: string;
}

export interface SheetRow {
  value: number;
  texts: SheetText[];
  problems: number;
}

export interface SheetBase {
  color: string;
  fill: boolean;
  /** A tool layer's outlines (a jig's pockets): drawn dashed, never burned. */
  tool: boolean;
  d: string;
}

export interface Sheet {
  rows: SheetRow[];
  next: number | null;
  total: number;
  start: number;
  end: number;
  step: number;
  targets: { id: string; name: string; mode: "csv" | "serial"; container: string | null }[];
  base: SheetBase[];
  stock: string | null;
  viewBox: [number, number, number, number] | null;
  problems: Partial<Record<FlagKind, number>>;
}

/** Rows per page the grid asks for. */
export const PAGE = 48;

export function sheetRequest(from: number | null, problemsOnly: boolean, count = PAGE): Invocation {
  const params: Record<string, unknown> = { count, outlines: true, problems_only: problemsOnly };
  if (from !== null) params.from = from;
  return { id: TXT.contactSheet, params };
}

const num = (v: unknown, d = 0) => (typeof v === "number" && Number.isFinite(v) ? v : d);
const str = (v: unknown) => (typeof v === "string" ? v : "");

export function parseSheet(r: unknown): Sheet {
  const o = r && typeof r === "object" ? (r as Record<string, unknown>) : {};
  const flags = (v: unknown): Flag[] =>
    Array.isArray(v)
      ? v.flatMap((f) => {
          if (!f || typeof f !== "object") return [];
          const x = f as Record<string, unknown>;
          return typeof x.kind === "string" ? [{ kind: x.kind as FlagKind, message: str(x.message) }] : [];
        })
      : [];
  const rows: SheetRow[] = Array.isArray(o.rows)
    ? o.rows.flatMap((row) => {
        if (!row || typeof row !== "object") return [];
        const x = row as Record<string, unknown>;
        const texts: SheetText[] = Array.isArray(x.texts)
          ? x.texts.flatMap((t) => {
              if (!t || typeof t !== "object") return [];
              const y = t as Record<string, unknown>;
              return [{ id: str(y.id), shown: str(y.shown), flags: flags(y.flags), d: typeof y.d === "string" ? y.d : undefined }];
            })
          : [];
        return [{ value: num(x.value), texts, problems: num(x.problems) }];
      })
    : [];
  const targets = Array.isArray(o.targets)
    ? o.targets.flatMap((t) => {
        if (!t || typeof t !== "object") return [];
        const x = t as Record<string, unknown>;
        const c = x.container && typeof x.container === "object" ? (x.container as Record<string, unknown>) : null;
        const container = c ? (c.kind === "object" ? str(c.name) || null : c.kind === "stock" ? "stock" : null) : null;
        return [{ id: str(x.id), name: str(x.name), mode: x.mode === "csv" ? ("csv" as const) : ("serial" as const), container }];
      })
    : [];
  const base: SheetBase[] = Array.isArray(o.base)
    ? o.base.flatMap((b) => (b && typeof b === "object" && typeof (b as SheetBase).d === "string" ? [{ color: str((b as SheetBase).color) || "currentColor", fill: (b as SheetBase).fill === true, tool: (b as SheetBase).tool === true, d: (b as SheetBase).d }] : []))
    : [];
  const vb = Array.isArray(o.view_box) && o.view_box.length === 4 && o.view_box.every((n) => typeof n === "number") ? (o.view_box as [number, number, number, number]) : null;
  const sum = o.summary && typeof o.summary === "object" ? (o.summary as Record<string, unknown>) : {};
  const pr = sum.problems && typeof sum.problems === "object" ? (sum.problems as Record<string, unknown>) : {};
  const problems: Partial<Record<FlagKind, number>> = {};
  for (const [k, v] of Object.entries(pr)) if (typeof v === "number") problems[k as FlagKind] = v;
  return {
    rows,
    next: typeof o.next === "number" ? o.next : null,
    total: num(o.total),
    start: num(o.start),
    end: num(o.end),
    step: num(o.step, 1),
    targets,
    base,
    stock: typeof o.stock === "string" ? o.stock : null,
    viewBox: vb,
    problems,
  };
}

/** Another page added to what is shown (the base, the stock and the view box stay those of the first page). */
export function appendPage(a: Sheet, b: Sheet): Sheet {
  const problems = { ...a.problems };
  for (const [k, v] of Object.entries(b.problems)) problems[k as FlagKind] = (problems[k as FlagKind] ?? 0) + (v ?? 0);
  const viewBox = a.viewBox && b.viewBox ? union(a.viewBox, b.viewBox) : (a.viewBox ?? b.viewBox);
  return { ...a, rows: [...a.rows, ...b.rows], next: b.next, problems, viewBox };
}

function union(a: [number, number, number, number], b: [number, number, number, number]): [number, number, number, number] {
  const x0 = Math.min(a[0], b[0]);
  const y0 = Math.min(a[1], b[1]);
  const x1 = Math.max(a[0] + a[2], b[0] + b[2]);
  const y1 = Math.max(a[1] + a[3], b[1] + b[3]);
  return [x0, y0, x1 - x0, y1 - y0];
}

/** The kinds a row has, most serious first, once each. */
export function rowKinds(row: SheetRow): FlagKind[] {
  const seen = new Set(row.texts.flatMap((t) => t.flags.map((f) => f.kind)));
  return FLAG_KINDS.filter((k) => seen.has(k));
}

/** A card's title: the row number and what its first text shows. */
export function rowTitle(row: SheetRow): string {
  const first = row.texts.find((t) => t.shown.trim())?.shown ?? "";
  return first.split("\n")[0];
}
