// The guide's build flag: the future design-only web build (docs/design-only/plan.md) has no laser, so it leaves out
// the pages with `needs: machine`. Set VITE_OPENETCH_DESIGN_ONLY=1 when building it. Under the Node test runner
// import.meta.env does not exist: the full guide.

export function designOnlyBuild(): boolean {
  try {
    return import.meta.env?.VITE_OPENETCH_DESIGN_ONLY === "1";
  } catch {
    return false;
  }
}
