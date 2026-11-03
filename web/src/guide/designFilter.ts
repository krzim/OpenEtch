// The guide as OpenEtch Design bundles it (vite.config.ts `designGuide`, docs/design-only/plan.md): the pages with
// `needs: machine` and the pages about features the design build does not have (MCP) keep only their front matter,
// turned into `needs: machine` (so the viewer leaves them out, as flags.ts says), and lines naming what must not ship
// (check-design-dist.mjs: MCP's address, LightBurn bundles) are dropped. The full app's guide is untouched. Pure, no
// imports: the Vite config and `npm test` both load it.

/** What the design bundle must not contain (scripts/check-design-dist.mjs checks the built files for the same). */
export const DESIGN_FORBIDDEN = ["/api/machine/jog", "/api/runs", "/mcp", "lbzip", "LightBurnSoftware.json", "import-cutlib"] as const;

/** Pages that need no laser but describe what the design build leaves out. */
export const DESIGN_HIDDEN_PAGES: readonly string[] = ["ai-agents"];

/** The page `slug`'s markdown as the design build bundles it. */
export function designGuideSource(slug: string, src: string): string {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(src);
  const front = m ? m[1] : "";
  const body = m ? src.slice(m[0].length) : src;
  const machine = /^needs:\s*machine\s*$/m.test(front) || DESIGN_HIDDEN_PAGES.includes(slug);
  if (machine) {
    const meta = front
      .split(/\r?\n/)
      .filter((l) => !/^needs:/.test(l))
      .concat("needs: machine")
      .join("\n");
    return `---\n${meta}\n---\n`;
  }
  const kept = body.split("\n").filter((line) => !DESIGN_FORBIDDEN.some((s) => line.includes(s)));
  return `${m ? m[0] : ""}${kept.join("\n")}`;
}
