// English strings of the design-only build's backend (src/standalone/): notices about the in-browser editor and its
// storage. Spread into the English table (en.ts). `{name}` marks a placeholder.

export const EN_STANDALONE = {
  "ds.restartedRecovered": "The editor restarted; recovered autosave from {time}",
  "ds.restartedNothing": "The editor restarted; there was no autosave to recover",
  "ds.reloadRecovered": "Your unsaved design from {time} is back",
  "ds.storageMemory": "This browser window cannot keep files: projects are lost when the tab closes. Download a copy to keep your work.",
  "ds.failed": "The editor could not start: {error}",
} as const;
