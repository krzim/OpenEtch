// The user guide's route: `#/guide` (the start page, docs/guide/index.md), `#/guide/<slug>` and
// `#/guide/<slug>#<anchor>`. Opening the guide pushes a history entry (the browser's Back goes back through the pages
// read, then closes it); every entry records how many guide entries lie before it, so Close returns to exactly where
// the app was. `?guide` in the address opens the guide alone, full width (Open in new tab). Small and in the main
// bundle; the viewer itself is a lazy chunk (GuideHost.tsx). Parsing is pure (route.test.ts).

export interface GuideRoute {
  slug: string;
  anchor: string | null;
}

export const GUIDE_EVENT = "openetch:guide";
/** Sent when the guide is asked to open (F1, Help > User guide, a "?"): the viewer then takes the focus, also when it
 * was open already at that page, so Esc closes it straight away. */
export const GUIDE_FOCUS_EVENT = "openetch:guide-focus";
const SLUG = "[a-z0-9][a-z0-9-]*";
// A heading's id, as GitHub makes it from the heading's text (guide/markdown.ts slugify): letters of any script,
// digits, "_" and "-". Kept here rather than imported, so the main bundle does not take in the markdown parser.
const ANCHOR = String.raw`[\p{L}\p{M}\p{Nd}\p{Pc}-]+`;
const HASH_RE = new RegExp(`^#/guide(?:/(${SLUG})?)?(?:#(${ANCHOR}))?$`, "u");
const TARGET_RE = new RegExp(`^(${SLUG})?(?:#(${ANCHOR}))?$`, "u");

/** The route in a location hash, or null when the hash is not a guide route. */
export function parseGuideHash(hash: string): GuideRoute | null {
  let h = hash;
  try {
    h = decodeURI(hash);
  } catch {
    // A malformed escape: match it as it is.
  }
  const m = HASH_RE.exec(h);
  if (!m) return null;
  return { slug: m[1] ?? "index", anchor: m[2] ?? null };
}

/** A help target ("slug", "slug#anchor" or "#anchor" on the start page) as a route; null when malformed. */
export function parseTarget(target: string): GuideRoute | null {
  const m = TARGET_RE.exec(target.trim());
  if (!m) return null;
  return { slug: m[1] ?? "index", anchor: m[2] ?? null };
}

export function guideHash(r: GuideRoute): string {
  if (r.slug === "index") return r.anchor ? `#/guide#${r.anchor}` : "#/guide";
  return `#/guide/${r.slug}${r.anchor ? `#${r.anchor}` : ""}`;
}

/** The address of the guide alone (Open in new tab). */
export function standaloneUrl(r: GuideRoute, loc: { origin: string; pathname: string } = location): string {
  return `${loc.origin}${loc.pathname}?guide${guideHash(r)}`;
}

/** Whether this page load is the guide alone (`?guide`). */
export function isStandalone(search: string = typeof location === "undefined" ? "" : location.search): boolean {
  return new URLSearchParams(search).has("guide");
}

function depthNow(): number {
  const s = typeof history === "undefined" ? null : (history.state as { oeGuide?: unknown } | null);
  return typeof s?.oeGuide === "number" ? s.oeGuide : 0;
}

function notify(): void {
  window.dispatchEvent(new Event(GUIDE_EVENT));
}

/** How many guide pages lie in the history up to here (1 on the first page the guide opened at; 0 while closed). */
export function guideDepth(): number {
  return depthNow();
}

/** Show a route: a new history entry, or `replace` the current one (an anchor on the same page). The route already
 * shown (F1 pressed again, the same "?" twice) replaces its entry, so Back still leaves the guide in one press. */
export function navigateGuide(r: GuideRoute, o: { replace?: boolean } = {}): void {
  const cur = parseGuideHash(location.hash);
  const open = cur !== null;
  const same = open && cur.slug === r.slug && cur.anchor === r.anchor;
  const url = `${location.pathname}${location.search}${guideHash(r)}`;
  if ((o.replace || same) && open) history.replaceState({ ...(history.state ?? {}), oeGuide: depthNow() }, "", url);
  else history.pushState({ oeGuide: open ? depthNow() + 1 : 1 }, "", url);
  notify();
}

/** Open the guide (F1, Help > User guide, a "?" button) at `target` ("slug#anchor"); the start page by default. */
export function openGuide(target = "index"): void {
  const r = parseTarget(target) ?? { slug: "index", anchor: null };
  navigateGuide(r);
  window.dispatchEvent(new Event(GUIDE_FOCUS_EVENT));
}

/** Close the guide: back past the guide's own history entries, or drop the hash when the app was opened on one. */
export function closeGuide(): void {
  if (parseGuideHash(location.hash) === null) return;
  const d = depthNow();
  if (d > 0) history.go(-d);
  else {
    history.replaceState(null, "", `${location.pathname}${location.search}`);
    notify();
  }
}

/** Back one guide page; closes the guide at its first page. */
export function backGuide(): void {
  if (depthNow() > 1) history.back();
  else closeGuide();
}

export function subscribeGuide(fn: () => void): () => void {
  window.addEventListener("popstate", fn);
  window.addEventListener("hashchange", fn);
  window.addEventListener(GUIDE_EVENT, fn);
  return () => {
    window.removeEventListener("popstate", fn);
    window.removeEventListener("hashchange", fn);
    window.removeEventListener(GUIDE_EVENT, fn);
  };
}

export function guideHashNow(): string {
  return typeof location === "undefined" ? "" : location.hash;
}

/** The side panel's width beside the canvas (desktop and large tablets), from the window width. */
export function guidePaneWidth(viewW: number): number {
  return Math.round(Math.min(460, Math.max(340, viewW * 0.28)));
}

/** Where the guide shows: beside the canvas when the window leaves the canvas room for it, else a full sheet. */
export function guideMode(layout: string, viewW: number): "pane" | "sheet" {
  return layout === "phone" || viewW < 1000 ? "sheet" : "pane";
}
