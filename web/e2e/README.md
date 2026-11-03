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
  backdrops are not panels; the history marker sits on the timeline; a ribbon flyout's dropdown menu may overlap the
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
- `journeys.mjs` (`npm run e2e:journeys -- --url ... [--doc ...] [--only rotate,grbl]`): longer sequences on one
  device each, with a screenshot and a check per step, under `target/ux-evidence/journeys/`: a phone turned mid-task,
  sheets over an active tool, a long-press inside a pinch, select all and a marquee, text with the keyboard up, every
  dialog of the phone's More sheet, a GRBL device added, made active and back (phone and MacBook), the theme switched
  in the app, Beginner Mode, a very long project name, warnings and toasts together, framing in the iPad's Laser
  workspace, a MacBook at 1280 x 720 with every dock open, iPad split view and Slide Over, a job run on the simulator
  from the phone, context menus at the screen edges, a large UI scale, a desktop window resized mid-session, the iPad
  turned with the keyboard up, undo past the import and redo, ruler guides and the corner's Clear guides. The layout
  check passing is not the verdict: look at the screenshots.

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
