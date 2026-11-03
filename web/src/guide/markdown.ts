// The user guide's markdown (docs/guide/*.md, docs/guide/README.md has the page format): a small, dependency-free
// parser to a plain tree that guide/render.tsx turns into React elements (never HTML strings, so nothing in a page can
// inject markup). It covers what the guide uses: front matter, headings (ids made from their text exactly as GitHub
// makes them, so `page.md#id` links jump both here and on GitHub), paragraphs,
// bold / italic / strikethrough, inline code, fenced code, ordered / unordered / nested lists, links, images, GitHub
// pipe tables, block quotes (callouts when they start with **Note:**, **Tip:** or **Warning:**) and rules. Raw HTML is
// not markup here: it stays text. Pure; tested in markdown.test.ts.

export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "strong"; c: Inline[] }
  | { t: "em"; c: Inline[] }
  | { t: "del"; c: Inline[] }
  | { t: "link"; href: string; c: Inline[] }
  | { t: "img"; src: string; alt: string }
  | { t: "br" };

export type Align = "left" | "center" | "right" | null;
export type CalloutKind = "note" | "tip" | "warning";

export type Block =
  | { t: "heading"; level: number; id: string; c: Inline[] }
  | { t: "para"; c: Inline[] }
  | { t: "code"; lang: string; v: string }
  | { t: "list"; ordered: boolean; start: number; items: Block[][]; tight: boolean }
  | { t: "quote"; kind: CalloutKind | null; c: Block[] }
  | { t: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  | { t: "hr" };

export interface FrontMatter {
  [key: string]: string;
}

export interface ParsedDoc {
  meta: FrontMatter;
  blocks: Block[];
}

// --- front matter ------------------------------------------------------------------------------------------------

/** Split `---`-fenced front matter (simple `key: value` lines) from the body. */
export function splitFrontMatter(src: string): { meta: FrontMatter; body: string } {
  const text = src.replace(/^﻿/, "").replace(/\r\n?/g, "\n");
  const m = /^---\n([\s\S]*?)\n---[ \t]*(?:\n|$)/.exec(text);
  if (!m) return { meta: {}, body: text };
  const meta: FrontMatter = {};
  for (const line of m[1].split("\n")) {
    const kv = /^([A-Za-z][\w-]*)\s*:\s*(.*)$/.exec(line);
    if (!kv) continue;
    let v = kv[2].trim();
    if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) v = v.slice(1, -1);
    meta[kv[1]] = v;
  }
  return { meta, body: text.slice(m[0].length) };
}

// --- slugs -------------------------------------------------------------------------------------------------------

/** A heading's id as GitHub makes it (its TableOfContentsFilter, and github-slugger, which matches it): the heading's
 *  rendered text (code spans, emphasis and link text count; an image's alt text does not, as it is not text on the
 *  page), lower-cased (Unicode-aware, so "É" -> "é"), every character that is not a letter, mark, number, connector
 *  (`_`), hyphen or space removed, then each space a hyphen. Nothing is trimmed or collapsed: "A & B" -> "a--b".
 *  Non-ASCII letters stay (GitHub keeps them; the link's href may carry them raw or percent-encoded). */
export function slugify(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^\p{L}\p{M}\p{N}\p{Pc} -]/gu, "")
    .replace(/ /g, "-");
}

/** The text GitHub slugs for a heading: like inlineText, without image alt text. */
export function headingText(c: readonly Inline[]): string {
  let s = "";
  for (const n of c) {
    if (n.t === "text" || n.t === "code") s += n.v;
    else if (n.t === "br") s += " ";
    else if (n.t !== "img") s += headingText(n.c);
  }
  return s;
}

/** A unique heading id on a page, as github-slugger numbers duplicates: the first "x" is "x", then "x-1", "x-2"; a
 *  candidate already taken (a heading that reads "x-1") moves on to the next number. `used` holds the page's ids. */
export function uniqueSlug(base: string, used: Map<string, number>): string {
  let id = base;
  while (used.has(id)) {
    const n = (used.get(base) ?? 0) + 1;
    used.set(base, n);
    id = `${base}-${n}`;
  }
  used.set(id, 0);
  return id;
}

/** The characters of a heading id, for a RegExp with the `u` flag (`%` too: an anchor in a link may be
 *  percent-encoded). */
export const ANCHOR_CHARS = String.raw`[\p{L}\p{M}\p{N}\p{Pc}%-]+`;

/** The plain text of inline content (alt text for images, no markup). */
export function inlineText(c: readonly Inline[]): string {
  let s = "";
  for (const n of c) {
    if (n.t === "text" || n.t === "code") s += n.v;
    else if (n.t === "img") s += n.alt;
    else if (n.t === "br") s += " ";
    else s += inlineText(n.c);
  }
  return s;
}

/** The plain text of blocks (search, summaries). */
export function blocksText(bs: readonly Block[]): string {
  const out: string[] = [];
  for (const b of bs) {
    if (b.t === "heading" || b.t === "para") out.push(inlineText(b.c));
    else if (b.t === "code") out.push(b.v);
    else if (b.t === "list") for (const it of b.items) out.push(blocksText(it));
    else if (b.t === "quote") out.push(blocksText(b.c));
    else if (b.t === "table") for (const r of [b.head, ...b.rows]) out.push(r.map(inlineText).join(" "));
  }
  return out.filter(Boolean).join("\n");
}

// --- inline ------------------------------------------------------------------------------------------------------

const PUNCT = /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/;
const isSpace = (c: string | undefined) => c === undefined || /\s/.test(c);
const isWordChar = (c: string | undefined) => c !== undefined && /[\p{L}\p{N}]/u.test(c);

/** Find the `]` closing the `[` at `open`, skipping code spans, escapes and nested brackets. */
function closeBracket(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    const c = s[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (c === "`") {
      const run = /^`+/.exec(s.slice(i))![0];
      const end = s.indexOf(run, i + run.length);
      if (end > 0) i = end + run.length - 1;
      continue;
    }
    if (c === "[") depth++;
    else if (c === "]" && --depth === 0) return i;
  }
  return -1;
}

/** Parse `(dest "title")` at `i` (the `(`): the destination and the index after `)`. */
function linkDest(s: string, i: number): { href: string; end: number } | null {
  if (s[i] !== "(") return null;
  let j = i + 1;
  while (s[j] === " ") j++;
  let href = "";
  if (s[j] === "<") {
    const e = s.indexOf(">", j);
    if (e < 0) return null;
    href = s.slice(j + 1, e);
    j = e + 1;
  } else {
    let depth = 0;
    const st = j;
    for (; j < s.length; j++) {
      const c = s[j];
      if (c === "\\") {
        j++;
        continue;
      }
      if (c === "(") depth++;
      else if (c === ")") {
        if (depth === 0) break;
        depth--;
      } else if (c === " ") break;
    }
    href = s.slice(st, j);
  }
  while (s[j] === " ") j++;
  // An optional title ("..." or '...'), ignored.
  if (s[j] === '"' || s[j] === "'") {
    const q = s[j];
    const e = s.indexOf(q, j + 1);
    if (e < 0) return null;
    j = e + 1;
    while (s[j] === " ") j++;
  }
  if (s[j] !== ")") return null;
  return { href: href.replace(/\\(.)/g, "$1"), end: j + 1 };
}

/** The closing delimiter for emphasis `d` (`*`, `_`, `**`, `__`, `~~`) opened before `from`, or -1. */
function closeDelim(s: string, from: number, d: string): number {
  for (let i = from; i < s.length; i++) {
    const c = s[i];
    if (c === "\\") {
      i++;
      continue;
    }
    if (c === "`") {
      const run = /^`+/.exec(s.slice(i))![0];
      const end = s.indexOf(run, i + run.length);
      if (end > 0) i = end + run.length - 1;
      continue;
    }
    if (c === "[") {
      // Skip a whole link or image: its text has emphasis of its own.
      const close = closeBracket(s, i);
      if (close > 0) {
        const dest = linkDest(s, close + 1);
        if (dest) {
          i = dest.end - 1;
          continue;
        }
      }
    }
    if (!s.startsWith(d, i)) continue;
    const single = d.length === 1;
    // A single delimiter must not be half of a double one (`**` inside `*...*`).
    if (single && (s[i + 1] === d || s[i - 1] === d)) {
      if (s[i + 1] === d) i++;
      continue;
    }
    if (i === from) continue;
    if (isSpace(s[i - 1])) continue;
    if (d[0] === "_" && isWordChar(s[i + d.length])) continue;
    return i;
  }
  return -1;
}

/** Inline markdown to nodes. Text is kept as text: raw HTML is not interpreted. */
export function parseInline(s: string): Inline[] {
  const out: Inline[] = [];
  let buf = "";
  const flush = () => {
    if (buf) out.push({ t: "text", v: buf });
    buf = "";
  };
  for (let i = 0; i < s.length; ) {
    const c = s[i];
    // Backslash escapes; a backslash at the end of a line is a hard break.
    if (c === "\\") {
      if (s[i + 1] === "\n") {
        flush();
        out.push({ t: "br" });
        i += 2;
        continue;
      }
      if (s[i + 1] !== undefined && PUNCT.test(s[i + 1])) {
        buf += s[i + 1];
        i += 2;
        continue;
      }
    }
    if (c === "`") {
      const run = /^`+/.exec(s.slice(i))![0];
      const end = s.indexOf(run, i + run.length);
      if (end > 0 && s[end + run.length] !== "`") {
        flush();
        let v = s.slice(i + run.length, end).replace(/\n/g, " ");
        if (/^ .*[^ ].* $/.test(v)) v = v.slice(1, -1);
        out.push({ t: "code", v });
        i = end + run.length;
        continue;
      }
      buf += run;
      i += run.length;
      continue;
    }
    if (c === "!" && s[i + 1] === "[") {
      const close = closeBracket(s, i + 1);
      const dest = close > 0 ? linkDest(s, close + 1) : null;
      if (dest) {
        flush();
        out.push({ t: "img", src: dest.href, alt: inlineText(parseInline(s.slice(i + 2, close))) });
        i = dest.end;
        continue;
      }
    }
    if (c === "[") {
      const close = closeBracket(s, i);
      const dest = close > 0 ? linkDest(s, close + 1) : null;
      if (dest) {
        flush();
        // No links inside links: an inner one keeps only its text.
        const inner = parseInline(s.slice(i + 1, close)).flatMap((n) => (n.t === "link" ? n.c : [n]));
        out.push({ t: "link", href: dest.href.trim(), c: inner });
        i = dest.end;
        continue;
      }
    }
    // ***both*** / ___both___: bold and italic at once.
    if ((c === "*" || c === "_") && s.startsWith(c.repeat(3), i) && !isSpace(s[i + 3]) && s[i + 3] !== c) {
      const end = s.indexOf(c.repeat(3), i + 3);
      if (end > i + 3 && !isSpace(s[end - 1])) {
        flush();
        out.push({ t: "strong", c: [{ t: "em", c: parseInline(s.slice(i + 3, end)) }] });
        i = end + 3;
        continue;
      }
    }
    if (c === "*" || c === "_" || (c === "~" && s[i + 1] === "~")) {
      const double = s[i + 1] === c;
      const d = double ? c + c : c;
      const after = s[i + d.length];
      const leftFlanking = !isSpace(after) && !(c === "_" && isWordChar(s[i - 1]));
      if (leftFlanking && !(double && s[i + 2] === c)) {
        const end = closeDelim(s, i + d.length, d);
        if (end > 0) {
          flush();
          const inner = parseInline(s.slice(i + d.length, end));
          out.push(c === "~" ? { t: "del", c: inner } : double ? { t: "strong", c: inner } : { t: "em", c: inner });
          i = end + d.length;
          continue;
        }
      }
      buf += d;
      i += d.length;
      continue;
    }
    // Two trailing spaces before a line break: a hard break; otherwise a soft break is a space.
    if (c === "\n") {
      if (/ {2,}$/.test(buf)) {
        buf = buf.replace(/ +$/, "");
        flush();
        out.push({ t: "br" });
      } else buf = buf.replace(/ +$/, "") + " ";
      i++;
      while (s[i] === " ") i++;
      continue;
    }
    buf += c;
    i++;
  }
  flush();
  return out;
}

// --- blocks ------------------------------------------------------------------------------------------------------

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/;
const HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const ITEM = /^( *)([-*+]|\d{1,9}[.)])( +|$)(.*)$/;
const TABLE_SEP = /^ *\|? *:?-+:? *(\| *:?-+:? *)*\|? *$/;

/** Split a pipe-table row into cells (escaped `\|` stays in the cell; pipes in code spans count, as on GitHub). */
export function tableCells(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|") && !s.endsWith("\\|")) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = "";
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "\\" && s[i + 1] === "|") {
      cur += "|";
      i++;
    } else if (s[i] === "|") {
      cells.push(cur.trim());
      cur = "";
    } else cur += s[i];
  }
  cells.push(cur.trim());
  return cells;
}

function isTableStart(lines: string[], i: number): boolean {
  return i + 1 < lines.length && lines[i].includes("|") && TABLE_SEP.test(lines[i + 1]) && lines[i + 1].includes("-");
}

/** Whether `line` starts a block that ends a paragraph. */
function startsBlock(lines: string[], i: number): boolean {
  const l = lines[i];
  return FENCE.test(l) || HEADING.test(l) || HR.test(l) || QUOTE.test(l) || ITEM.test(l) || isTableStart(lines, i);
}

const CALLOUT = /^(Note|Tip|Warning):$/;

function calloutOf(c: Block[]): CalloutKind | null {
  const first = c[0];
  if (first?.t !== "para") return null;
  const s = first.c[0];
  if (s?.t !== "strong") return null;
  const m = CALLOUT.exec(inlineText(s.c).trim());
  return m ? (m[1].toLowerCase() as CalloutKind) : null;
}

/** Parse markdown blocks. `used` collects the heading ids so far (duplicates get -1, -2 as on GitHub; see uniqueSlug). */
export function parseBlocks(src: string, used: Map<string, number> = new Map()): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").replace(/\t/g, "    ").split("\n");
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const mark = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !new RegExp(`^ {0,3}${mark[0]}{${mark.length},}\\s*$`).test(lines[i])) body.push(lines[i++]);
      i++;
      out.push({ t: "code", lang: fence[2] ?? "", v: body.join("\n") });
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      // `{#custom-id}` is not markdown on GitHub (it shows as text), so it is not here either.
      const text = (h[2] ?? "").replace(/[ \t]+#+$/, "").replace(/^#+$/, "");
      const c = parseInline(text.trim());
      const id = uniqueSlug(slugify(headingText(c)), used);
      out.push({ t: "heading", level: h[1].length, id, c });
      i++;
      continue;
    }
    if (HR.test(line)) {
      out.push({ t: "hr" });
      i++;
      continue;
    }
    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length && lines[i].trim()) {
        const q = QUOTE.exec(lines[i]);
        // A lazy continuation line (no `>`) continues the quote's paragraph.
        if (!q && startsBlock(lines, i)) break;
        body.push(q ? q[1] : lines[i]);
        i++;
      }
      const c = parseBlocks(body.join("\n"), used);
      out.push({ t: "quote", kind: calloutOf(c), c });
      continue;
    }
    if (isTableStart(lines, i)) {
      const head = tableCells(lines[i]);
      const align: Align[] = tableCells(lines[i + 1]).map((c) => (c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : c.startsWith(":") ? "left" : null));
      const rows: Inline[][][] = [];
      i += 2;
      while (i < lines.length && lines[i].trim() && lines[i].includes("|")) {
        const cells = tableCells(lines[i]);
        rows.push(head.map((_, k) => parseInline(cells[k] ?? "")));
        i++;
      }
      out.push({ t: "table", align: head.map((_, k) => align[k] ?? null), head: head.map((c) => parseInline(c)), rows });
      continue;
    }
    const item = ITEM.exec(line);
    if (item) {
      const r = parseList(lines, i, used);
      out.push(r.block);
      i = r.next;
      continue;
    }
    // Paragraph.
    const para: string[] = [line.trimStart()];
    i++;
    while (i < lines.length && lines[i].trim() && !startsBlock(lines, i)) para.push(lines[i++].trimStart());
    out.push({ t: "para", c: parseInline(para.join("\n").trimEnd()) });
  }
  return out;
}

/** A list starting at `start`: items of the same kind at the same indent, their bodies parsed as blocks. */
function parseList(lines: string[], start: number, used: Map<string, number>): { block: Block; next: number } {
  const first = ITEM.exec(lines[start])!;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const markOf = (m: RegExpExecArray) => (/\d/.test(m[2]) ? m[2].slice(-1) : m[2]);
  const mark = markOf(first);
  const sameList = (k: number) => {
    const m = k < lines.length ? ITEM.exec(lines[k]) : null;
    return !!m && m[1].length === indent && /\d/.test(m[2]) === ordered && markOf(m) === mark;
  };
  const items: Block[][] = [];
  let tight = true;
  let i = start;
  while (sameList(i)) {
    const m = ITEM.exec(lines[i])!;
    // Content column: where the item's text starts (the marker plus its spaces; one space when the line is empty).
    const pad = m[3].length > 4 || m[4] === "" ? 1 : m[3].length;
    const contentCol = indent + m[2].length + pad;
    const body: string[] = [m[4]];
    i++;
    while (i < lines.length) {
      const l = lines[i];
      if (!l.trim()) {
        body.push("");
        i++;
        continue;
      }
      if (/^ */.exec(l)![0].length >= contentCol) {
        body.push(l.slice(contentCol));
        i++;
        continue;
      }
      // A lazy paragraph continuation: no blank line before it and not the start of another block.
      if (body[body.length - 1].trim() && !startsBlock(lines, i)) {
        body.push(l.trim());
        i++;
        continue;
      }
      break;
    }
    // Trailing blank lines lie between this item and the next.
    let gap = false;
    while (body.length > 1 && !body[body.length - 1].trim()) {
      body.pop();
      gap = true;
    }
    const blocks = parseBlocks(body.join("\n"), used);
    // Loose: a blank line between items, or between two of an item's own paragraphs.
    if ((gap && sameList(i)) || (body.some((b) => !b.trim()) && blocks.filter((b) => b.t !== "list").length > 1)) tight = false;
    items.push(blocks);
  }
  return { block: { t: "list", ordered, start: ordered ? parseInt(first[2], 10) : 1, items, tight }, next: i };
}

/** Parse a whole page: front matter, then the body. */
export function parseDoc(src: string): ParsedDoc {
  const { meta, body } = splitFrontMatter(src);
  return { meta, blocks: parseBlocks(body) };
}

// --- links -------------------------------------------------------------------------------------------------------

export type LinkTarget =
  | { kind: "page"; slug: string; anchor: string | null }
  | { kind: "anchor"; anchor: string }
  | { kind: "external"; href: string }
  | { kind: "invalid" };

const PAGE_LINK = new RegExp(String.raw`^(?:\./)?([a-z0-9][a-z0-9-]*)\.md(?:#(${ANCHOR_CHARS}))?$`, "u");
const ANCHOR_LINK = new RegExp(`^#(${ANCHOR_CHARS})$`, "u");

/** A link's `#anchor` with percent-escapes decoded (GitHub accepts `#caf%C3%A9` and `#café` alike). */
function decodeAnchor(a: string): string {
  try {
    return decodeURIComponent(a);
  } catch {
    return a;
  }
}

/** Classify a link destination: another guide page (`slug.md#anchor`), an anchor on this page, or an external http(s)
 *  / mailto link. Anything else (javascript:, data:, paths out of docs/guide) is invalid and is shown as text. */
export function linkTarget(href: string): LinkTarget {
  const h = href.trim();
  let m = PAGE_LINK.exec(h);
  if (m) return { kind: "page", slug: m[1], anchor: m[2] === undefined ? null : decodeAnchor(m[2]) };
  m = ANCHOR_LINK.exec(h);
  if (m) return { kind: "anchor", anchor: decodeAnchor(m[1]) };
  if (/^(https?:\/\/[^\s/$.?#][^\s]*|mailto:[^\s@]+@[^\s@]+)$/i.test(h)) return { kind: "external", href: h };
  return { kind: "invalid" };
}

/** An image source the guide allows: a file in docs/guide/img/. Returns the file name, or null. */
export function imageName(src: string): string | null {
  const m = /^(?:\.\/)?img\/([a-z0-9][a-z0-9-]*\.(?:webp|png|jpe?g|gif|svg))$/.exec(src.trim());
  return m ? m[1] : null;
}

/** Every link and image in blocks (the docs test checks them). */
export function collectRefs(bs: readonly Block[]): { links: string[]; images: string[] } {
  const links: string[] = [];
  const images: string[] = [];
  const walkInline = (c: readonly Inline[]) => {
    for (const n of c) {
      if (n.t === "link") {
        links.push(n.href);
        walkInline(n.c);
      } else if (n.t === "img") images.push(n.src);
      else if (n.t === "strong" || n.t === "em" || n.t === "del") walkInline(n.c);
    }
  };
  const walk = (b: readonly Block[]) => {
    for (const x of b) {
      if (x.t === "heading" || x.t === "para") walkInline(x.c);
      else if (x.t === "list") x.items.forEach(walk);
      else if (x.t === "quote") walk(x.c);
      else if (x.t === "table") [x.head, ...x.rows].forEach((r) => r.forEach(walkInline));
    }
  };
  walk(bs);
  return { links, images };
}
