// The guide's build flags: OpenEtch Design (the browser-only build, docs/design-only/plan.md, src/mode.ts) has no
// laser, so it leaves out the pages with `needs: machine` and the pages designFilter.ts hides. vite.config.ts lists
// those pages' slugs at build time (`__OPENETCH_GUIDE_HIDDEN__`), so a "?" that would open one is not shown there.
// Under the Node test runner: the full guide.

import { DESIGN_ONLY } from "../mode.ts";

declare const __OPENETCH_GUIDE_HIDDEN__: readonly string[] | undefined;

const HIDDEN: ReadonlySet<string> = new Set(typeof __OPENETCH_GUIDE_HIDDEN__ !== "undefined" ? __OPENETCH_GUIDE_HIDDEN__ : []);

export function designOnlyBuild(): boolean {
  return DESIGN_ONLY;
}

/** Whether this build leaves out the guide page of `target` ("page" or "page#anchor"). */
export function hiddenInBuild(target: string): boolean {
  return HIDDEN.has(target.split("#")[0]);
}
