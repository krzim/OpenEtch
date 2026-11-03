// The guide as OpenEtch Design bundles it (vite.config.ts `designGuide`, docs/design-only/plan.md): the pages with
// `needs: machine` and the pages about features the design build does not have (MCP) keep only their front matter,
// turned into `needs: machine` (so the viewer leaves them out, as flags.ts says), and lines naming what must not ship
// (check-design-dist.mjs: MCP's address, LightBurn bundles) or about AI agents over MCP are dropped. With the list of
// hidden pages, a list item pointing at one goes too (the contents), and a link to one in the text keeps its words but
// loses the link. The full app's guide is untouched. Pure, no imports: the Vite config and `npm test` both load it.

/** What the design bundle must not contain (scripts/check-design-dist.mjs checks the built files for the same). */
export const DESIGN_FORBIDDEN = ["/api/machine/jog", "/api/runs", "/mcp", "lbzip", "LightBurnSoftware.json", "import-cutlib"] as const;

/** Lines with these words describe what the design build does not have (agents over MCP): left out. */
export const DESIGN_DROPPED_WORDS: readonly string[] = ["MCP"];

/** Pages that need no laser but describe what the design build leaves out. */
export const DESIGN_HIDDEN_PAGES: readonly string[] = ["ai-agents"];

const FRONT = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/;

/** Whether OpenEtch Design leaves page `slug` out (`needs: machine`, or DESIGN_HIDDEN_PAGES). */
export function hiddenInDesign(slug: string, src: string): boolean {
  const m = FRONT.exec(src);
  return DESIGN_HIDDEN_PAGES.includes(slug) || (!!m && /^needs:\s*machine\s*$/m.test(m[1]));
}

/** The slugs of the pages OpenEtch Design leaves out, from [slug, markdown] pairs. */
export function designHiddenPages(pages: readonly (readonly [string, string])[]): string[] {
  return pages
    .filter(([slug, src]) => hiddenInDesign(slug, src))
    .map(([slug]) => slug)
    .sort();
}

/** A list item that only points at one of `hidden` (`- [Title](page.md): ...`, a contents or "see also" line). */
function listsHiddenPage(line: string, hidden: ReadonlySet<string>): boolean {
  const m = /^\s*[-*] \[[^\]]*\]\(([A-Za-z0-9_-]+)\.md(?:#[^)]*)?\)/.exec(line);
  return !!m && hidden.has(m[1]);
}

/** `[words](hidden-page.md#x)` becomes `words`: the page is not in this build. */
function unlinkHidden(line: string, hidden: ReadonlySet<string>): string {
  return line.replace(/\[([^\]]*)\]\(([A-Za-z0-9_-]+)\.md(?:#[^)]*)?\)/g, (all: string, words: string, slug: string) => (hidden.has(slug) ? words : all));
}

/**
 * The page `slug`'s markdown as the design build bundles it. `hidden` (the slugs the build leaves out) also drops the
 * list items that point at those pages, such as the contents on the guide's first page, and unlinks the others.
 */
export function designGuideSource(slug: string, src: string, hidden: readonly string[] = []): string {
  const leftOut = new Set(hidden);
  const m = FRONT.exec(src);
  const front = m ? m[1] : "";
  const body = m ? src.slice(m[0].length) : src;
  if (hiddenInDesign(slug, src)) {
    const meta = front
      .split(/\r?\n/)
      .filter((l) => !/^needs:/.test(l))
      .concat("needs: machine")
      .join("\n");
    return `---\n${meta}\n---\n`;
  }
  const dropped: readonly string[] = [...DESIGN_FORBIDDEN, ...DESIGN_DROPPED_WORDS];
  const kept = body
    .split("\n")
    .filter((line) => !dropped.some((s) => line.includes(s)) && !listsHiddenPage(line, leftOut))
    .map((line) => unlinkHidden(line, leftOut));
  return `${m ? m[0] : ""}${kept.join("\n")}`;
}
