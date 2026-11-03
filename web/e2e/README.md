# Device-emulation harness

Walks the UI on emulated phones, tablets and desktops, takes a screenshot after every step and checks the layout for
overlapping controls, controls off screen or in the safe-area bands, small touch targets and a hidden STOP. No npm
packages: Node 22 (global `WebSocket` and `fetch`) drives headless Microsoft Edge or Chrome over the Chrome DevTools
Protocol.

```sh
# 1. An engine with the simulator on a spare port (never a real machine: the flows start and stop framing, and they
#    replace the engine's document).
openetch --config-dir <tmp>/cfg --projects-dir <tmp>/projects --listen 127.0.0.1:7931 --approval click --web-dir web/dist

# 2. The matrix (all devices, each in its own themes), screenshots and reports under target/ux-evidence/.
cd web
npm run e2e:devices -- --url http://127.0.0.1:7931 [--doc some-heavy-file.lbrn2] [--devices iphone,ipad] \
  [--theme dark|light|all] [--steps job,zoom] [--out ../target/ux-evidence/my-run] [--cdp-port 9333] [--headful]
```

Without `--doc` a generated SVG of a few hundred shapes is loaded. Keep private documents and their screenshots out of
the repository: the default output folder is under `target/`, which git ignores. Set `E2E_BROWSER` to use another
Chromium-based browser.

## Performance profile

`perf.mjs` (`npm run e2e:perf -- --url ... --doc heavy.lbrn2`) walks the everyday actions on a heavy document on a
MacBook (1470 x 832 @2) and an iPhone 15 profile: the first load, select, move, Fill Region hover and fill, undo, redo,
a QR code, a text, an SVG and a PNG import, a timeline rollback four steps back and forward again, Design to Laser (the
preview), a layer setting and an undo in the Laser workspace, framing start and stop, back to Design. For each action it
records the wall time until the UI settles (no request of the action in flight and nothing for `--quiet` ms), every
request (method, path, bytes on the wire, server time, transfer time), main-thread long tasks, `JSON.parse` time of
payloads over 50 KB and the renderer's task / script / layout time. Before the walk it times the engine's big GETs
directly and how far gzip and brotli would shrink them. Actions the phone layout reaches differently are sent as the
same engine request from inside the page (`via: "api"` in the report), so the app's reaction is still what is measured.

```sh
# An engine with the SIMULATOR (the walk replaces the document, edits it and starts framing). On a Pi: a separate
# engine on a spare port with its own --config-dir and --projects-dir, never the one that drives the laser.
openetch --config-dir <tmp>/cfg --projects-dir <tmp>/projects --listen 0.0.0.0:7881 --no-token --approval click --web-dir <dist>

cd web
npm run e2e:perf -- --url http://<pi-ip>:7881 --doc some-heavy-file.lbrn2 --label pi-wifi \
  [--devices macbook-air-1470,iphone15-portrait] [--only load,undo,to-laser-preview] [--quiet 1500] [--timeout 240000]
```

Results: `target/perf/<label>/perf.json` and `perf.md` (git ignores `target/`). With `--only` the page still loads and
the workspaces still switch, but only the named actions are timed and the setup of the others is skipped.

The quick loop for trying a change: a local simulator engine, the four actions that matter most, and the network
throttled to Wi-Fi speed (`--throttle-mbps 20` is about 2.5 MB/s, with `--latency-ms 5` per request by default), so the
numbers reflect a Pi over Wi-Fi without one. It takes well under a minute:

```sh
npm run e2e:perf -- --url http://127.0.0.1:7931 --doc heavy.lbrn2 --label quick --throttle-mbps 20 \
  --only move,undo,to-laser-preview,layer-speed
```

The preview worker's jobs (src/wasm/, `window.__oeWorker`) hold an action open while they run and are listed with it.
`--no-worker` turns the worker off (the app then downloads the engine's preview), for a before / after comparison.

`node e2e/syncstorm.mjs http://127.0.0.1:7931 60 1` checks the document change sets against a local simulator engine
(never the Pi: it edits freely): rounds of concurrent edits, undo/redo, timeline moves, edits from another client and
document switches, with the browser's copy compared to `GET /api/document` after each round.

## Text and template screens

`templates.mjs` (`node e2e/templates.mjs --url http://127.0.0.1:7931 [--devices ...]`) opens the example projects
and walks the fill-in form, text auto-fit, the font picker's favourites and samples, the template fields dialog, the
contact sheet and New from example on an iPhone, an iPad and a MacBook (screenshots and layout checks under
`target/ux-evidence/templates/`). It replaces the engine's document, so run it against a simulator engine.

## Guide screenshots

`guide-shots.mjs` (`npm run guide:shots`) takes the user guide's pictures: it reads `docs/guide/shots.json` and writes
`docs/guide/img/<name>.webp` (WebP at quality 82, at most 1600 px wide unless a shot sets `maxWidth`). Pages show them
as `![Alt text](img/<name>.webp)`; `npm test` fails when a page uses a picture that is not listed or not taken yet, or a
listed shot that no page uses. After a run into `docs/guide/img/` it rewrites `docs/guide/img-sizes.json` (each picture's pixel size, which the guide gives its `<img>` so pages do not jump while pictures load).

```sh
# Let the script start its own simulator engine (temporary --config-dir and --projects-dir, no LightBurn prefs,
# --approval click, --no-token, 127.0.0.1:7971), from a built web/dist:
npm run build
npm run guide:shots -- --engine ../target/debug/openetch [--port 7971] [--only guide-beside-canvas,cuts-layers-coin]

# Or point it at a simulator engine you started (on this computer, never ports 7878 / 7979):
openetch --config-dir <tmp>/cfg --projects-dir <tmp>/projects --lightburn-prefs <tmp>/none.ini \
  --listen 127.0.0.1:7971 --approval click --no-token --web-dir web/dist
npm run guide:shots -- --url http://127.0.0.1:7971

npm run guide:shots -- --check        # validate shots.json only (no browser, no engine)
```

Public-safe by construction: shots open only the built-in examples (`doc.new {example}`); `--doc`, any path outside the
repository and engine commands that read or write files are refused; the engine must report itself as a simulator on
loopback and must not have imported this computer's LightBurn devices; the light theme is the default; the browser's
clock is fixed at 15 January 2026 10:30 UTC (en-GB, UTC time zone); before the capture the engine's address becomes
`openetch.local` and home paths become `C:\Users\you` / `/home/you`, and a shot is refused when a home path, a LAN
address, this computer's name or the login name is still on screen. Times the engine itself makes (job log entries)
are not fixed: keep those out of pictures.

A shot: `{ "name": "kebab-case", "device": "<devices.mjs id>", "theme"?: "light" | "dark", "example": "<built-in
example id>", "steps": [...], "crop"?: "<CSS selector>", "cropWidth"?: CSS px, "cropPad"?: CSS px, "maxWidth"?: px, "note"?: "what it shows"
}` (`cropWidth` keeps only the left part of a long crop, such as the timeline's first steps; `cropPad` is the margin round the crop, 8 px by default). Each step is an
object with exactly one action key:

| Step | What it does |
|---|---|
| `{ "exec": "layer.update", "params": { ... } }` | Run an engine command (as the UI, `/api/execute`); file commands are refused |
| `{ "click": "Accessible name" }` | Press a control by its aria-label, text or title (exact, then prefix, then substring); `"in": ".selector"` narrows it |
| `{ "click": { "css": "tr.cuts-row", "text": "Rim" } }` | Press an element by selector (and text) |
| `{ "click": { "object": "Name" } }` | Press a shape on the canvas by its name or id (checked against the Browser's selected row) |
| `{ "click": { "point": [17.25, 0] } }` | Press a point of the field, in mm |
| `{ "set": "#c08a00", "into": target }` | Set a select, colour or date field (one a person sets with a native picker) |
| `{ "hover": target }` | Move the mouse there (desktop devices); as the last step the pointer stays for the capture |
| `{ "dblclick": target }` / `{ "context": target }` | Double-click (two taps), or right-click (long-press); `{ "context": "selection" }` opens the selection's menu |
| `{ "menu": ["File", "Laser", "Devices"] }` | Open a menu path (desktop ribbon / top bar) |
| `{ "type": "text", "into"?: target }` | Type into the focused field, or into `into` (its text is selected first) |
| `{ "key": "Ctrl+Shift+Z" }` | A key with modifiers (`Ctrl`, `Alt`, `Shift`, `Meta`/`Cmd`); `Escape`, `Enter`, `Delete`, arrows, `F1`... |
| `{ "hash": "#/guide/the-workspace#the-ribbon" }` | Go to a hash route (the guide, `#/cockpit`) |
| `{ "wait": ".selector" }` / `{ "wait": 500 }` | Wait for a visible element (up to 10 s), or a number of ms |
| `{ "select": "all" }`, `"none"` or `["Name", "obj-3"]` | Select shapes (several need a keyboard: desktop only) |
| `{ "workspace": "design" }` / `"laser"` | Switch workspace (on a phone: the Canvas / Job tab) |
| `{ "tab": "Layers", "sheet"?: "half" }` | Phone: a bottom tab, its sheet at `peek`, `half`, `full` or `closed` |
| `{ "escape": true }` | Close menus, dialogs and drawers |
| `{ "scroll": ".selector" }` | Scroll that element to the top of its scrolling panel |

Every shot starts in a fresh browser context: `doc.new {example}`, the page loaded, the example's notes and the "Use
as an app" offer closed. Writers who describe a shot in words ("the Cuts / Layers window on a PC, two-tone coin, Rim
selected") turn it into steps one action at a time: the device and example become `device` and `example`, "open X"
becomes `click` or `menu` with the label exactly as the UI shows it, "select Y" becomes `select`, "on the phone's
Layers tab" becomes `tab`, a setting becomes `exec` with the engine command's parameters, "showing just the panel"
becomes `crop`. `npm test` checks every step's shape; run the shot to check that it works.

## Files

- `devices.mjs`: the device matrix (CSS viewport, DPR, touch, user agent, safe-area insets, themes, keyboard height).
  iPhone 15 Safari portrait with the bars shown / collapsed / as a Home Screen app, landscape, iPhone SE, Pixel 8,
  iPad Air portrait and landscape (iPadOS reports a Mac with touch points), MacBook Air 13 at 1470x832 and 1280x720,
  Windows at 1920x969 @1.25 and 2560x1307 @1.5.
- `cdp.mjs`: launch the browser, one isolated browser context per page, and helpers: `setDevice`, `goto`,
  `evaluate`, `tap`, `press` (tap or click), `longPress`, `drag` (finger or mouse), `pinch` (two touch points),
  `wheel`, `key`, `type`, `setViewportHeight`, `screenshot`.
- `overlap.mjs`: `checkLayout`, run in the page. It collects the visible controls (buttons, inputs, selects, menu items,
  tabs, ...) and the floating panels (absolutely or fixed positioned boxes with a surface: canvas overlays, toolbars,
  toasts, sheets, menus), clipped by their scroll containers, and reports
  - `stop` (critical): STOP missing or not hit-testable at its centre and corners;
  - `overlap`: two boxes that intersect, neither containing the other, where the one on top at the shared area
    (`elementFromPoint`) is not an intended cover;
  - `offscreen`: partly or wholly outside the viewport;
  - `safe-area`: inside the device's safe-area bands;
  - `small`: touch targets under 44 x 44 px on touch devices (a check box or radio button counts its `<label>`'s box,
    which takes the tap too).

  Intended layers (the allowlist): menus, list boxes, popovers, ribbon flyouts, tooltips and modal dialogs may cover
  anything; the phone's bottom sheet and the tablet's dock overlays may cover the canvas and its overlays; modal
  backdrops are not panels; the canvas viewport is the page at any size (the user guide beside it can make it small); the history marker sits on the timeline; a ribbon flyout's dropdown menu may overlap the
  flyout's edge. A covered control that pokes out above its cover is still reported. What a toast covers is reported one step lower (medium): it goes away by itself. A pinch that zooms the whole page
  is reported as `page-zoom` (critical when STOP leaves the visible area), and the harness then resets the zoom.
- `scenarios.mjs`: the flows. Every device: first visit, empty document, heavy document fitted, a selection, the
  Rectangle tool (and a rectangle drawn) and the Line tool with the touch modifier bar, a context menu (right-click or
  long-press), framing started and stopped, an object dragged out of the galvo field (the off-field warning), zoom in
  and out (wheel or pinch). Desktop: Design and Laser workspaces, File / Edit / Window menus, Devices and Preferences
  dialogs, Cut settings. Tablet: the rail groups, the dock overlays, the Properties pill, the on-screen keyboard. Phone:
  the Canvas, Layers, Job and More tabs with their sheet at peek, half and full, Preferences and Devices from More, the
  Job tab with its sheet closed and a selection (the reference overlap case), the on-screen keyboard (the viewport
  shrinks by the keyboard height while a field has focus, and the bottom safe-area band goes: the keyboard covers the
  home indicator).
- `production.mjs` (`node e2e/production.mjs --url ... [--devices ...]`): the production screens on the iPad (both
  orientations), the iPhone and a MacBook: the cockpit idle, waiting at a layer pause and after Continue, the foot
  switch cycle framing, the Laser workspace's Production window, the maintenance chip and the serial ledger, and the
  design lock refusing an edit. It needs a simulator engine with `--approval click` and a foot switch (it presses
  Start, the simulated pedal and STOP and replaces the document); output under `target/ux-evidence/production/`.
- `journeys.mjs` (`npm run e2e:journeys -- --url ... [--doc ...] [--only rotate,grbl]`): longer sequences on one
  device each, with a screenshot and a check per step, under `target/ux-evidence/journeys/`: a phone turned mid-task,
  sheets over an active tool, a long-press inside a pinch, select all and a marquee, text with the keyboard up, every
  dialog of the phone's More sheet, a GRBL device added, made active and back (phone and MacBook), the theme switched
  in the app, Beginner Mode, a very long project name, warnings and toasts together, framing in the iPad's Laser
  workspace, a MacBook at 1280 x 720 with every dock open, iPad split view and Slide Over, a job run on the simulator
  from the phone, context menus at the screen edges, a large UI scale, a desktop window resized mid-session, the iPad
  turned with the keyboard up, undo past the import and redo, ruler guides and the corner's Clear guides, and the Material Library's Edit Cut changing a preset's pauses for the
  operator on an iPhone, an iPad, a MacBook and a PC (`library-edit-cut-<device>`, a generic library it loads and
  unloads). The layout
  check passing is not the verdict: look at the screenshots.

- `guide-shots.mjs`, `guideSteps.mjs`: the guide's screenshots and their step language (above). `scenarios.mjs`'s
  `guide-*` steps (`--steps guide`) open the guide on every device (F1, More > User guide, the phone's More tab), search
  it, open a result and close it.

- `history.mjs` (`node e2e/history.mjs --url ... [--doc ...] [--devices ...]`): runs a few jobs on the simulator
  (the owner's file if given, a variable-text tag and a generated 3D Sliced coin), marks them OK / scrap, then on a
  phone, an iPad and a MacBook opens the 3D relief preview (whole job, half way, low view), the Job log window (list,
  a run, search, Storage) and Re-run as burned (it checks that a pending run appeared and nothing started), with a
  screenshot and the layout check per step, under `target/ux-evidence/history-*/`.

- `links.mjs` (`node e2e/links.mjs --url ... [--devices ...]`): where the feature packages meet. It lays a serial
  number into a 1 x 3 tag jig with pocket 2 empty plus a tag whose number is too long, burns it once on the simulator,
  then on an iPad, an iPhone and a MacBook shows the cockpit's slot chips with the pocket numbers, the pre-flight
  warning from the contact sheet's check of the next row, and the Job log's run with each value's pocket and the
  jig's pockets. Output under `target/ux-evidence/links/`.

Every script that checks a touch device sets the `--safe-*` tokens to its insets (`applySafeArea`) after loading the
page; without them the layout check reports the bottom rows as inside the safe-area band although the app would pad
them out of it.

## Emulation limits

- Safe areas: headless Chromium reports `env(safe-area-inset-*)` as 0. The app reads them only through the `--safe-*`
  tokens (`src/tokens.css`), so the harness sets those tokens to the device's insets. A new direct `env()` use would
  not be emulated.
- The on-screen keyboard is approximated by shrinking the layout viewport; iOS Safari shrinks the visual viewport
  instead and may scroll the page, so check keyboard findings on a device.
- Pointer media queries (`pointer: coarse`, `hover: none`) follow touch emulation; Safari's own behaviour (rubber-band
  scrolling, the bars collapsing while scrolling, double-tap zoom) is not emulated.
- Rendering uses the machine's GPU. Under a software renderer (SwiftShader) a heavy document blocks the main thread for
  seconds, and timing-dependent gestures (a drag turning into a long-press) behave differently.
