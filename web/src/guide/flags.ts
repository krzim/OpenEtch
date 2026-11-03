// The guide's build flag: OpenEtch Design (the browser-only build, docs/design-only/plan.md, src/mode.ts) has no
// laser, so it leaves out the pages with `needs: machine`. Under the Node test runner: the full guide.

import { DESIGN_ONLY } from "../mode.ts";

export function designOnlyBuild(): boolean {
  return DESIGN_ONLY;
}
