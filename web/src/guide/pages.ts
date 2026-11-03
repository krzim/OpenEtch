// The user guide as data: pages from docs/guide/*.md (front matter + parsed body), the contents grouped by section,
// previous / next, the sections a "?" button can link to, and search over titles, headings and text. Pure (the
// markdown comes in as strings: guide/content.ts bundles it, the tests read it with fs); tested in pages.test.ts.

import { blocksText, inlineText, parseDoc, type Block } from "./markdown.ts";

/** The guide's sections, in reading order (front matter `section:`). */
export const SECTIONS = ["Getting started", "Designing", "Layers and materials", "Running jobs", "Production", "Setup and reference"] as const;
export type Section = (typeof SECTIONS)[number];

/** `needs: machine`: only meaningful with a laser connected (the design-only build leaves these pages out). */
export type Needs = "design" | "machine";

export interface Heading {
  id: string;
  level: number;
  text: string;
}

export interface GuidePage {
  slug: string;
  title: string;
  order: number;
  section: Section;
  needs: Needs;
  summary: string;
  blocks: Block[];
  headings: Heading[];
  /** Problems with the front matter (the docs test fails on them; the viewer still shows the page). */
  problems: string[];
}

export interface Guide {
  pages: GuidePage[];
  bySlug: ReadonlyMap<string, GuidePage>;
}

/** The slug of a docs/guide file path ("../../../docs/guide/fit-to-coin.md" -> "fit-to-coin"), or null for files
 *  that are not pages (README.md, the writers' notes). */
export function slugOfPath(path: string): string | null {
  const m = /(?:^|[\\/])([^\\/]+)\.md$/.exec(path);
  if (!m || m[1] === "README") return null;
  return m[1];
}

export function parsePage(slug: string, src: string): GuidePage {
  const { meta, blocks } = parseDoc(src);
  const problems: string[] = [];
  if (!/^[a-z0-9][a-z0-9-]*$/.test(slug)) problems.push(`file name "${slug}.md" is not kebab-case`);
  const title = meta.title?.trim() ?? "";
  if (!title) problems.push("front matter has no title");
  const order = Number(meta.order);
  if (!meta.order || !Number.isFinite(order)) problems.push(`order "${meta.order ?? ""}" is not a number`);
  const section = (SECTIONS as readonly string[]).includes(meta.section ?? "") ? (meta.section as Section) : null;
  if (!section) problems.push(`section "${meta.section ?? ""}" is not one of: ${SECTIONS.join(", ")}`);
  const needs = meta.needs === "machine" ? "machine" : "design";
  if (meta.needs !== "design" && meta.needs !== "machine") problems.push(`needs "${meta.needs ?? ""}" is not design or machine`);
  const summary = meta.summary?.trim() ?? "";
  if (!summary) problems.push("front matter has no summary");
  for (const k of Object.keys(meta)) if (!["title", "order", "section", "needs", "summary"].includes(k)) problems.push(`unknown front matter key "${k}"`);
  const headings: Heading[] = blocks.flatMap((b) => (b.t === "heading" ? [{ id: b.id, level: b.level, text: inlineText(b.c) }] : []));
  return {
    slug,
    title: title || slug,
    order: Number.isFinite(order) ? order : 9999,
    section: section ?? "Setup and reference",
    needs,
    summary,
    blocks,
    headings,
    problems,
  };
}

/** Build the guide from {path or slug: markdown}; pages sorted by section, order, then title. */
export function buildGuide(files: Readonly<Record<string, string>>): Guide {
  const pages: GuidePage[] = [];
  for (const [path, src] of Object.entries(files)) {
    const slug = path.endsWith(".md") ? slugOfPath(path) : path;
    if (slug) pages.push(parsePage(slug, src));
  }
  pages.sort((a, b) => SECTIONS.indexOf(a.section) - SECTIONS.indexOf(b.section) || a.order - b.order || a.title.localeCompare(b.title));
  return { pages, bySlug: new Map(pages.map((p) => [p.slug, p])) };
}

/** The pages a build shows: the design-only build leaves out `needs: machine`. */
export function visiblePages(g: Guide, designOnly: boolean): GuidePage[] {
  return g.pages.filter((p) => !designOnly || p.needs !== "machine");
}

/** The contents: sections in order, each with its pages (empty sections left out). */
export function contents(g: Guide, designOnly: boolean): { section: Section; pages: GuidePage[] }[] {
  const vis = visiblePages(g, designOnly);
  return SECTIONS.map((section) => ({ section, pages: vis.filter((p) => p.section === section) })).filter((s) => s.pages.length > 0);
}

/** The pages before and after `slug` in reading order. */
export function prevNext(g: Guide, slug: string, designOnly: boolean): { prev: GuidePage | null; next: GuidePage | null } {
  const vis = visiblePages(g, designOnly);
  const i = vis.findIndex((p) => p.slug === slug);
  if (i < 0) return { prev: null, next: null };
  return { prev: vis[i - 1] ?? null, next: vis[i + 1] ?? null };
}

/** Every anchor a link or a "?" button may target on a page (its heading ids). */
export function anchorsOf(p: GuidePage): Set<string> {
  return new Set(p.headings.map((h) => h.id));
}

/** Whether `target` ("slug" or "slug#anchor") names a page and, if given, one of its headings. */
export function resolveTarget(g: Guide, target: string): { ok: true } | { ok: false; why: string } {
  const m = /^([a-z0-9][a-z0-9-]*)(?:#([A-Za-z][\w-]*))?$/.exec(target);
  if (!m) return { ok: false, why: `"${target}" is not "page" or "page#anchor"` };
  const p = g.bySlug.get(m[1]);
  if (!p) return { ok: false, why: `no page docs/guide/${m[1]}.md` };
  if (m[2] && !anchorsOf(p).has(m[2])) return { ok: false, why: `no heading {#${m[2]}} in docs/guide/${m[1]}.md` };
  return { ok: true };
}

// --- search ------------------------------------------------------------------------------------------------------

/** A searchable part of a page: the text from one ## or ### heading to the next (the intro has the page's anchor). */
export interface SearchSection {
  slug: string;
  pageTitle: string;
  anchor: string | null;
  heading: string;
  text: string;
}

export function sectionsOf(p: GuidePage): SearchSection[] {
  const out: SearchSection[] = [];
  let cur: SearchSection = { slug: p.slug, pageTitle: p.title, anchor: null, heading: p.title, text: p.summary };
  let body: Block[] = [];
  const close = () => {
    const t = blocksText(body);
    cur.text = [cur.text, t].filter(Boolean).join("\n");
    out.push(cur);
  };
  for (const b of p.blocks) {
    if (b.t === "heading" && b.level === 1) continue;
    if (b.t === "heading" && (b.level === 2 || b.level === 3)) {
      close();
      cur = { slug: p.slug, pageTitle: p.title, anchor: b.id, heading: inlineText(b.c), text: "" };
      body = [];
      continue;
    }
    body.push(b);
  }
  close();
  return out;
}

export interface SearchHit extends SearchSection {
  score: number;
  /** Some text around the first match. */
  snippet: string;
}

/** Lower case, accents off, punctuation as spaces. */
export function fold(s: string): string {
  return s
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}

function snippetOf(text: string, tokens: string[], width = 140): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const low = fold(flat);
  // fold keeps the length only roughly (punctuation collapses): find the match in the folded text, then cut the
  // original around the same proportion.
  let at = -1;
  for (const t of tokens) {
    const k = low.indexOf(t);
    if (k >= 0 && (at < 0 || k < at)) at = k;
  }
  if (at < 0 || flat.length <= width) return flat.slice(0, width) + (flat.length > width ? "…" : "");
  const pos = Math.round((at / Math.max(1, low.length)) * flat.length);
  const start = Math.max(0, pos - Math.round(width / 3));
  const s = flat.slice(start, start + width);
  return `${start > 0 ? "…" : ""}${s}${start + width < flat.length ? "…" : ""}`;
}

/** Search the guide: every word must appear (in the title, the heading or the text); headings and titles weigh more. */
export function searchGuide(g: Guide, query: string, designOnly: boolean, limit = 30): SearchHit[] {
  const tokens = fold(query).split(" ").filter(Boolean);
  if (!tokens.length) return [];
  const hits: SearchHit[] = [];
  for (const p of visiblePages(g, designOnly))
    for (const s of sectionsOf(p)) {
      const title = ` ${fold(s.pageTitle)} `;
      const heading = ` ${fold(s.heading)} `;
      const text = ` ${fold(s.text)} `;
      let score = 0;
      let all = true;
      for (const t of tokens) {
        const word = (h: string) => h.includes(` ${t}`);
        let ts = 0;
        if (heading.includes(t)) ts += word(heading) ? 8 : 4;
        if (title.includes(t)) ts += word(title) ? 4 : 2;
        if (text.includes(t)) ts += word(text) ? 2 : 1;
        if (!ts) {
          all = false;
          break;
        }
        score += ts;
      }
      if (!all) continue;
      // The page's own top section ranks a little above its parts for the same words.
      if (s.anchor === null) score += 1;
      hits.push({ ...s, score, snippet: snippetOf(s.text, tokens) });
    }
  hits.sort((a, b) => b.score - a.score || a.pageTitle.localeCompare(b.pageTitle));
  return hits.slice(0, limit);
}

// --- the start page's contents (GitHub readers have no viewer) --------------------------------------------------------

/** The heading that starts the generated contents of index.md (everything after it is rewritten). */
export const CONTENTS_HEADING = "## Contents {#contents}";

/** The contents list of index.md: every other page by section, with its summary. */
export function contentsMarkdown(g: Guide): string {
  const out: string[] = [CONTENTS_HEADING, ""];
  for (const s of contents(g, false)) {
    const pages = s.pages.filter((p) => p.slug !== "index");
    if (!pages.length) continue;
    out.push(`**${s.section}**`, "");
    for (const p of pages) out.push(`- [${p.title}](${p.slug}.md): ${p.summary}${p.needs === "machine" ? " (Needs a laser)" : ""}`);
    out.push("");
  }
  return out.join("\n");
}

/** index.md with its contents section regenerated (appended when it has none). */
export function withContents(indexSrc: string, g: Guide): string {
  const text = indexSrc.replace(/\r\n?/g, "\n");
  const at = text.indexOf(`\n${CONTENTS_HEADING}`);
  const head = (at >= 0 ? text.slice(0, at) : text).replace(/\s+$/, "");
  return `${head}\n\n${contentsMarkdown(g)}`;
}
