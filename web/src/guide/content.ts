// The guide's pages and pictures, bundled by Vite from docs/guide (the source of truth, also read on GitHub): the
// markdown as strings, the images as asset URLs. Imported only by the lazy viewer chunk (GuideViewer.tsx), so none of
// it is in the app's start-up bundle; the images download only when a page shows them (loading="lazy").

import { buildGuide, type Guide } from "./pages.ts";

const RAW = import.meta.glob("../../../docs/guide/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const IMAGES = import.meta.glob("../../../docs/guide/img/*.{webp,png,jpg,jpeg,gif,svg}", { query: "?url", import: "default", eager: true }) as Record<string, string>;

const IMAGE_BY_NAME = new Map(Object.entries(IMAGES).map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1), url]));
// Each picture's pixel size, written by guide-shots.mjs (docs.test.ts checks it against the files).
const SIZES = Object.values(import.meta.glob("../../../docs/guide/img-sizes.json", { import: "default", eager: true }))[0] as Record<string, [number, number]> | undefined;

let guide: Guide | null = null;

/** The guide, parsed once. */
export function loadGuide(): Guide {
  guide ??= buildGuide(RAW);
  return guide;
}

/** The bundled URL of docs/guide/img/<name>, or undefined. */
export function imageUrl(name: string): string | undefined {
  return IMAGE_BY_NAME.get(name);
}

/** [width, height] in pixels of docs/guide/img/<name>, or undefined. */
export function imageSize(name: string): readonly [number, number] | undefined {
  return SIZES?.[name];
}
