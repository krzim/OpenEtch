// Light, Dark and "Match system" themes (LightBurn's Application Theme). The theme is the `data-theme` attribute on
// <html>; styles.css defines every colour as a token per theme. Canvas strokes in layer colours are nudged towards
// white (dark theme) or black (light theme) only when they would be nearly invisible on the field background, so a
// black C00 layer still reads on a dark canvas and the colour stays recognisable.

import type { ThemePref } from "./prefs.ts";

export type Theme = "light" | "dark";

/** Field (canvas work area) background per theme; keep in step with `--field` in styles.css. */
export const FIELD_BG: Record<Theme, string> = { light: "#ffffff", dark: "#1d2024" };

export function resolveTheme(pref: ThemePref, systemDark: boolean): Theme {
  return pref === "system" ? (systemDark ? "dark" : "light") : pref;
}

/** [r, g, b] 0..255 of "#rgb" or "#rrggbb", or null for anything else (named colours, rgb(), ...). */
export function parseHex(c: string): [number, number, number] | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(c.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? [...m[1]].map((x) => x + x).join("") : m[1];
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

export function toHex(rgb: [number, number, number]): string {
  return `#${rgb.map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0")).join("")}`;
}

/** WCAG relative luminance, 0 (black) to 1 (white). */
export function luminance(rgb: [number, number, number]): number {
  const ch = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * ch(rgb[0]) + 0.7152 * ch(rgb[1]) + 0.0722 * ch(rgb[2]);
}

/** WCAG contrast ratio, 1 to 21. */
export function contrast(a: [number, number, number], b: [number, number, number]): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/**
 * Minimum contrast a layer stroke keeps against the field: on the dark field even pure blue needs lifting; on the
 * white one only very pale colours change, so LightBurn's colours stay as they are.
 */
export const MIN_STROKE_CONTRAST: Record<Theme, number> = { light: 1.5, dark: 3 };

/**
 * The colour to stroke a layer's shapes with on the canvas: the layer colour, mixed towards white on the dark field
 * or black on the light one just enough to reach `MIN_STROKE_CONTRAST`. Non-hex colours are returned unchanged.
 */
export function canvasColor(color: string | undefined, theme: Theme): string {
  const fallback = theme === "dark" ? "#c8ccd2" : "#333333";
  if (!color) return fallback;
  const rgb = parseHex(color);
  if (!rgb) return color;
  const bg = parseHex(FIELD_BG[theme]) as [number, number, number];
  const min = MIN_STROKE_CONTRAST[theme];
  if (contrast(rgb, bg) >= min) return color;
  const target = theme === "dark" ? 255 : 0;
  for (let t = 0.1; t <= 1.0001; t += 0.1) {
    const mixed = rgb.map((v) => v + (target - v) * t) as [number, number, number];
    if (contrast(mixed, bg) >= min) return toHex(mixed);
  }
  return fallback;
}
