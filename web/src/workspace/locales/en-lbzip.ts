// English strings of LightBurn bundles (.lbzip), which only the full app has: their manifest's item names were
// inferred from LightBurn's executable, so OpenEtch Design leaves the format out altogether (docs/design-only/plan.md
// §4). en.ts spreads this table only outside the design build (mode.ts DESIGN_ONLY), after EN_FILES, whose
// "files.bundlesHint" it extends.

export const EN_LBZIP = {
  "files.exportLbzip": "Export LightBurn bundle (.lbzip)",
  "files.exportLbzipTitle": "A bundle for LightBurn (File > Bundles > Import Bundle): devices, material libraries, material test presets, image presets and art libraries. Preferences and hotkeys are not included.",
  "files.bundlesHint": "Moves your settings, devices and material libraries to another computer. Imported devices are added as new devices and keep this machine's safety limits. Import also reads LightBurn bundles (.lbzip): their settings (only the ones OpenEtch has), devices, material libraries, material test presets, image presets and art libraries (not hotkeys).",
} as const;
