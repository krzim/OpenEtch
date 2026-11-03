# The OpenEtch user guide

**Readers:** start at [index.md](index.md). The same pages open inside the app with `F1` or **Help > User guide**.

This folder is the source of the guide. Every `<slug>.md` here (except this README) is one page. The web app bundles the pages and the pictures in `img/` (src/guide/), and GitHub shows them as they are, so a page must read well in both places.

## Page format

A page starts with front matter:

```text
---
title: Fit a design to a coin
order: 20
section: Designing
needs: design
summary: Centre a design on round stock and size it to the coin.
---
```

- `title`: the page's name. The page's only `#` heading repeats it.
- `order`: a number. Pages are listed by section, then by order. Leave gaps (10, 20, 30) so pages can go in between.
- `section`: one of `Getting started`, `Designing`, `Layers and materials`, `Running jobs`, `Production`, `Setup and reference`.
- `needs`: `design`, or `machine` when the page only matters with a laser connected. Pages with `machine` get a **Needs a laser** tag, and the future design-only web build leaves them out.
- `summary`: one sentence, for the contents and for search.

Then the body:

- One `# Title`, then `##` for each task or section and `###` below that.
- A heading's anchor is the one GitHub gives it, made from its text: lower case, punctuation removed (except `-` and `_`), each space a hyphen. `## Cut settings` is `#cut-settings`, `## The Cuts / Layers window` is `#the-cuts--layers-window` (the `/` goes, both spaces stay). Code spans, bold and link text count as text. A heading that repeats one above it on the page gets `-1`, `-2`. Do not write `{#custom-id}` after a heading: GitHub shows it as text.
- The app's **?** buttons link to headings by these anchors, so give every panel, dialog and tool its own short heading (`## Hatch settings`, not `## Hatch: interval, angle and cross hatch`). Rewording a heading changes its anchor: `npm test` names every link and **?** button that no longer resolves, so fix them in the same change.
- Links to other pages: `[text](other-page.md#anchor)`, or `[text](#anchor)` on the same page. Websites: `https://...` (they open in a new tab). Nothing else (no `../` paths, no other schemes).
- Pictures: `![Alt text that says what the picture shows](img/<shot-name>.webp)`. The shot must be listed in `shots.json` (below). About one picture per major section, where it really helps: a panel, a dialog, a phone layout.
- Callouts: a block quote that starts with `**Note:**`, `**Tip:**` or `**Warning:**`.
- Keys: in code spans, `Ctrl+Z`. No raw HTML anywhere (no `<kbd>`, no comments).
- Tables: GitHub pipe tables. Fenced code blocks for commands.

The pages in this folder use every one of these; [the-workspace.md](the-workspace.md) and [getting-started.md](getting-started.md) are good models.

## Writing style

Plain English, British spelling (colour, centre, optimise), short sentences, second person, one idea per paragraph. Start with the task ("To fit a design to a coin:") and number the steps. Quote labels exactly as the app shows them, in bold, with menu paths like **File > Open** (the labels are in web/src/workspace/locales/en*.ts). When a step differs on a phone or an iPad, say where the control is there too. Do not overclaim: settings are starting points to test, not guarantees.

Laser safety where it matters: eye protection rated for the wavelength, fume extraction, never leave a job unattended, and the energy density of every test before you run it (power ÷ (speed × line interval), in J/mm²).

## Screenshots

`shots.json` lists every picture: its name, the emulated device, the built-in example it opens and the steps that get the screen ready. `npm run guide:shots` (in web/) takes them with a simulator engine and writes `img/<name>.webp`. The step language and the commands are in [web/e2e/README.md](../../web/e2e/README.md#guide-screenshots). Pictures show only the built-in examples or shapes drawn fresh, never your own files.

## Checks

`npm test` (in web/) fails when a page has bad front matter, a heading with a `{#custom-id}`, a link or a **?** button to a missing page or heading, a picture that is not in `shots.json` or not taken yet, a shot no page uses, raw HTML, or a contents list in index.md that is out of date (`npm run guide:index` rewrites it).
