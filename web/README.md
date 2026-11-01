# OpenEtch web UI

Fusion 360-style front end for the OpenEtch engine: React 18 + TypeScript + Vite, plain CSS, no runtime
dependencies beyond `react` and `react-dom`. It talks only to the engine API described in `../ARCHITECTURE.md`.

## Run

```sh
cd web
npm install

# Development: hot reload on http://localhost:5173 (also reachable from an iPad on the LAN).
# /api (including the /api/events WebSocket) and /mcp are proxied to the engine on 127.0.0.1:7878,
# so start the engine first.
npm run dev

# Production build into web/dist, which the engine serves.
npm run build

# Type-check only / unit tests (Node 22's built-in runner, no extra deps).
npm run typecheck
npm test
```

If the engine requires a token, open the UI once as `http://host:port/?token=...`. The token is saved in
`localStorage`, removed from the address bar, and sent as `Authorization: Bearer <token>` on every HTTP request. Only
the `/api/events` WebSocket carries it as `?token=` (browsers cannot set headers there); the engine refuses `?token=`
anywhere else.

## How it works

- **No local document edits.** Every change is `POST /api/execute` (or `/api/batch`) with `origin: "ui"`. The UI
  refetches `/api/document` and `/api/history` on the WebSocket `document_changed` event and, as a fallback, after
  every execute response. Undo/redo use `POST /api/undo` and `POST /api/redo`.
- **Command ids and parameter shapes** live in one file, `src/api/commands.ts`, matching the input structs in
  `crates/commands/src/builtin`. At startup the UI compares them against `GET /api/commands` and shows
  "N cmd missing" in the top bar (hover for the list) if the engine lacks any. The **Commands** toolbar button runs any
  registered document command with raw JSON parameters: the same catalogue an agent sees over MCP.
- **Viewport** draws in field millimetres with +Y up, as stacked layers that share one view transform (pan/zoom and
  the y-flip): field, grid and stock (SVG), images (canvas), object outlines and fills (SVG), the toolpath preview
  (canvas) and the interaction SVG on top (handles, marquee, canvas tools, snap glyphs). The scene layers are memoized
  on document and preview data, so a zoom or pan only changes their transform (strokes use `vector-effect:
  non-scaling-stroke`) and repaints the canvases' visible region at device-pixel resolution: images from cached
  ImageBitmaps (original and dithered preview; smoothing off once magnified past native resolution, so pixels stay
  crisp), the toolpath from segments uploaded once to WebGL2 (`components/toolpathGL.ts`; a tiled 2D canvas without
  it). Wheel, pinch and pan updates are coalesced to one per animation frame (`src/sceneLayers.ts`). The field
  square comes from `GET /api/machine` (`field_mm`), 153.75 mm until the engine reports it. Pointer Events throughout:
  drag empty space to pan, wheel or pinch to zoom, two fingers to pan, tap to select (shift adds), drag a selection to
  move it (one `object.transform` translate on release). Rectangle/Ellipse tools: drag to draw (Shift: square or
  circle), tap for 20 mm; Circle tool (C): press at the centre and drag the radius. A lone circle shows Diameter and
  "Keep round" in Properties (stored as an ellipse with equal radii).
- **Editing tools** (`components/EditTools.tsx`, LightBurn behaviour and hotkeys): Line (Ctrl+L; click points,
  double-click/Enter finishes, C or the first point closes, Shift 15°), Pen (drag a point for a smooth curve node),
  Polygon/star (drag from the centre; sides, star ratio and corner radius in the options bar), Rectangle/Ellipse Ctrl
  from the centre and Shift square. Edit nodes (Ctrl+`) lists the shape's nodes with `node.list`: drag nodes, handles
  or segments (live preview, `node.move` on release), click a segment to add a node, marquee/Shift selects; keys at
  the pointer or on the selection: Delete/D, B break, J join, S smooth, C corner, L line, K curve, I/M insert, T trim;
  rectangles and ellipses offer Convert to path first. Measure (distance, ΔX, ΔY, angle, snapped), Trim (click a
  piece). Modify and Arrange dropdowns: weld (Ctrl+W), union/subtract/intersect (Alt++ / Alt+- / Alt+*), XOR, cut
  shapes (Alt+C), offset (Alt+O), fillet, break apart (Alt+B), auto-join (Alt+J), close, delete duplicates (Alt+D),
  convert (Ctrl+Shift+C), optimize (Alt+Shift+O), reverse; draw order (PgUp/PgDn, Ctrl+PgUp/PgDn), lock, make same
  size, Transform (scale %, shear, mirror across a picked line, two-point rotate Ctrl+2 / scale), grid and circular
  arrays (centre pick), copy along path, quick nest. Rulers (in the display unit) run along the top and left; drag a
  guide out of a ruler (back onto it to remove; guides move with the Select tool). Guides are kept per project in this
  browser (the saved project's name, else the opened file, else one unsaved slot; Save carries them to the project
  and New clears the unsaved slot; guides an older version stored by document name are adopted once)
  (`src/guides.ts`) and are snap targets. The Viewport hands presses to these tools through `src/canvasTool.ts`.
- **Snapping** (`src/snapping.ts`, Fusion 360-style): centres, quadrants, vertices/endpoints, straight-segment
  midpoints, intersections between outlines near the cursor and the nearest point on an outline, plus stock, field and
  grid points, each with its own glyph and a label ("Quadrant · Circle 1"); dotted alignment guides when the point
  lines up with another snap point. 8 px tolerance; a dragged selection snaps by its own snap point nearest the grab;
  Ctrl turns snapping off, Alt snaps to guides only. Candidates sit in a grid hash and the per-move work is capped.
- **Passes (LightBurn sub-layers)**: with a layer selected, the Properties panel lists its passes in run order: "Main"
  (the layer's own settings, `layer.update`) and each sub-layer (`layer.sub_update`), with its name, a Cleanup badge
  and a settings summary. The open pass is edited with the full cut-settings form; Sub-layer, Cleanup pass (same mode
  and interval, a quarter of the power, twice the speed, one pass, hatch turned 45°, cleaning after the main passes),
  Duplicate, Delete and Move up/down are `layer.sub_add`, `layer.sub_duplicate`, `layer.sub_delete` and
  `layer.sub_move` (each one undo step; hidden on engines without them, where sub-layers are shown read-only). In the
  Toolpath card a layer with sub-layers gets one row per pass (settings, job passes, mark length, time) with show and
  solo, and the overlay colours each pass of a layer distinctly. A segment's pass comes from its `sub_layer`, else the
  preview's `passes[pass].sub_layer`, else by counting the layer's job passes against its settings (`api/passes.ts`).
- **Laser workspace**: toolpath overlay from `GET /api/preview` (marks coloured by layer, jumps dashed grey), machine
  status (with a "LightBurn devices" section: the devices from `GET /api/lightburn`, the one the engine matched, the
  profile differences from `GET /api/machine` `lightburn.diffs`, and "Load LightBurn settings…", which cuts the device
  list out of a picked prefs.ini in the browser and sends only that to `POST /api/lightburn/prefs`; the rest of the
  file, including LightBurn's secrets, never leaves the browser; "Forget" is `DELETE /api/lightburn/prefs`), runs
  under way, pending runs, and recent runs (finished, failed, aborted while streaming, or cancelled before
  it). **Frame** traces the job with the red pointer and **Stop frame**
  (`POST /api/machine/frame/stop`) ends it without touching pending runs. **STOP** (`POST /api/machine/stop`) is the
  emergency stop: it sits in the top bar in every workspace, is never gated, aborts any run or frame and cancels
  every run that has not started streaming.
- **Approval**: each pending run shows who requested it (You or the agent's name), its revision, estimated time,
  mark/jump length, segment count and packet count (the packet count only while pending). Approving takes two taps ("Approve…", then the operator PIN and "Confirm
  and fire"); the confirm step reverts after 20 s without typing. The PIN is set once on the engine machine with
  `openetch set-pin`; the UI keeps it only in component state while the confirm step is open and sends it only as
  `{"pin"}` to `POST /api/runs/{id}/approve`; the PIN field takes 6-12 digits. Stale runs cannot be approved: the
  engine's `stale` flag, or a run revision other than the document's (the machine status is also refetched on every
  `document_changed`). The engine's 403 codes `pin_not_set`, `pin_invalid`, `pin_required`, `pin_locked` and
  `pin_changed` get specific messages. While `approval.locked_until_unix_s` is set (or after an `approval_locked`
  event) the card counts down to the unlock and Approve is disabled; `approval.pin_changed` disables it with "PIN file
  changed - restart the engine". With a
  physical confirm input configured, an approved run shows "Press the foot pedal" while it is `awaiting_confirm`;
  STOP cancels it.
- **Events**: `run_finished` refreshes the machine and toasts (an error toast unless the run finished cleanly);
  `document_changed` with an agent or script `origin` flashes that edit's timeline chip. Undo/redo with nothing to do
  (200 `{ok:false}`) shows the engine's message as a toast.
- **File menu** (far left of the top bar, tap to open): New (`doc.new`), Open… (the engine's projects from
  `GET /api/projects`, opened with `POST /api/projects/open`, or "Upload from this device" for a `.openetch` or `.lbrn2`
  file), Save / Save as… (`POST /api/projects/save`; Save asks for a name the first time, and Save as asks before
  replacing an existing project: names are compared ignoring case, since Windows and macOS file names do, and Save as
  sends `overwrite: false` until the user taps Replace, so a 409 from the engine also turns into the Replace prompt),
  Import LightBurn… (`.lbrn2`, Replace or Merge, with the galvo field size the
  LightBurn project was laid out for, since `.lbrn2` files do not store it), Export to LightBurn and Download
  .openetch. **Ctrl/Cmd+S** saves (anywhere, even in a field). New, Open and a replacing import ask first when the
  project has unsaved changes (all of them stay undoable).
  - Uploads go to `POST /api/document/upload?format=...&mode=...` with the raw file as the body (50 MB at most).
  - LightBurn field size: the Open/Import dialog (also for dropped files) reads each file's root `DeviceName` in the
    browser (LightBurn 2.x writes the device's DisplayName, e.g. "JCZFiber (Unknown)") and shows the size the engine
    will use: the device named by the file, else the engine's LightBurn device (`--lightburn-device`, else LightBurn's
    default device), else the design's `field_mm`, else the machine's ("Field size 153.8 mm — from your LightBurn
    device JCZFiber", with an Edit link). Only an edited size is sent as `&field_mm=<mm>`; otherwise the engine applies
    the same rule and reports `field_source`, which the toast names. Export to LightBurn shows its default the same way
    (the size the LightBurn file was opened with, else the design's, else the machine's) and always sends the size
    shown. Exports and downloads after Open are named after the opened file until the design is saved as a project.
  - Downloads are fetched with the Authorization header and saved through a blob link, so they work with token auth:
    `.openetch` from `GET /api/document/download?format=openetch` (named by its Content-Disposition), `.lbrn2` from
    `doc.export_lbrn2` so its warnings can be shown.
  - Import and export warnings (unset frequencies, text imported as outlines, skipped bitmaps...) appear in a
    dismissible panel, identical lines folded with a count.
  - The top bar shows the project name and a dot while there are unsaved changes (`GET /api/project`, refetched on
    `document_changed` and `project_saved`). After New, an upload or a replacing import the document is shown as
    unsaved ("Untitled") and Save asks for a name, even if the engine still names the previous project, so the old
    project is never overwritten by accident.
- **Narrow screens** (< 1024 px, e.g. iPad portrait): the Browser and Properties panels become drawers toggled from the
  toolbar.

## Layout

```
src/
  api/types.ts        wire types (Document, HistoryEntry, Preview, MachineStatus...)
  api/client.ts       fetch wrapper, auth token, endpoints
  api/commands.ts     every command id + parameter builder the UI uses
  api/runs.ts         run lists, approval gate and error texts, stats rows, toasts (pure; tested)
  api/project.ts      project names, file names, import/export answers and warnings, dirty state (pure; tested)
  api/passes.ts       a layer's passes (main + sub-layers): pass list, cleanup defaults, per-pass preview (pure; tested)
  api/lightburn.ts    LightBurn devices, .lbrn2 DeviceName, the import field-size rule, prefs.ini device list (pure; tested)
  api/useEngine.ts    React hook: fetching, WebSocket events, refetch coalescing
  geometry.ts         affine maths, geometry -> SVG path, bounds (pure; tested)
  sceneLayers.ts      canvas layer maths: view matrix, grid spacing, image smoothing, per-frame view coalescing,
                      toolpath batches by style and tile (pure; tested)
  selection.ts        UI-only selection state (pure; tested)
  components/         TopBar, FileMenu, Toolbar, BrowserPanel, Viewport, PropertiesPanel, LaserPanel, LightBurn, Timeline, Dialogs, ui
  components/SceneCanvas.tsx, toolpathGL.ts   the Viewport's canvas layers: images (ImageBitmap cache), toolpath (WebGL2)
  components/EditTools.tsx, EditMenus.tsx, EditDialogs.tsx, Rulers.tsx, edit.css   editing tools, menus, dialogs, rulers
  api/geomCommands.ts geometry command builders (path.*, node.*, shape.polygon/line, arrays, order, lock...) (tested)
  nodes.ts, drawTools.ts, guides.ts   node model, drawing/measure/ruler maths, guides (pure; tested)
  workspace/          the workspace layer (styles in workspace/workspace.css):
    prefs.ts, usePrefs.ts       Preferences store (localStorage) and hooks: theme, units, grid, snapping, nudge, wheel,
                                UI scale, crosshair, language, hotkey overrides
    theme.ts, units.ts          Light/Dark/Match system, canvas stroke contrast; mm/in and mm/s, mm/min, in/s (display only)
    i18n.ts, locales/en.ts      message tables with English fallback; add a language with registerLocale or a new table
    hotkeys.ts, useKeymap.ts    LightBurn default key map, rebinding, cheat sheet data; the key layer in front of App/EditTools
    CutsLayers.tsx, PaletteBar.tsx, LaserDock.tsx, NumericBar.tsx, StatusBar.tsx, HelpMenu.tsx,
    PreferencesDialog.tsx, HotkeysDialog.tsx, RecentProjects.tsx, useWorkspace.tsx
```

### Workspace layer

- **Themes.** Every colour in `styles.css` is a token on `:root`, redefined for dark under
  `@media (prefers-color-scheme: dark)` (Match system) and `:root[data-theme="dark"]`. The top-bar button cycles
  Light, Dark, Match system; Preferences has the same choice. Layer strokes on the canvas go through
  `canvasColor()` so a black or dark-blue layer stays visible on the dark field.
- **Units.** `NumberField`/`PairField` (components/ui.tsx) show fields labelled `mm` or `mm/s` in the display unit and
  convert back on commit, so every command still carries millimetres.
- **Hotkeys.** `hotkeys.ts` lists every bindable action with LightBurn's defaults. Actions implemented by App.tsx or
  EditTools.tsx list their `native` chords (keep them in step); a rebound key is re-sent as the native chord, so the
  handlers stay the single implementation. F1 or `?` opens the cheat sheet; File > Edit hotkeys rebinds.
- **Docks.** Cuts / Layers (both workspaces) and Laser (Laser workspace) are collapsible docks in the right panel; the
  colour palette (C00-C29, T1, T2) and the numeric edits bar frame the canvas in the Design workspace; the status bar
  is the last row. The numeric bar's field groups wrap onto a second row and narrow in a narrow canvas column (a CSS
  size container), and its 9-point anchor is the same one Properties uses (`useAnchor` in `numeric.ts`, kept in this
  browser).
- **Layer order** is the cut order. Cuts / Layers reorders by drag (above or below a row) and up/down buttons, the
  Browser tree by drag and Move up / Move down in the layer menu, each one `layer.move {id, to_index}`; hidden on
  engines without it. After a palette swatch or the Layer field moves the selection, the highlighted layer follows it
  (`pruneSelection`).
- **Display units everywhere**: besides fields, the rulers, canvas readouts (cursor, field size, drag/size, node and
  Measure readouts), cut-settings summaries, Laser stats, hints and dialog messages use the length and speed units;
  dialogs share `UnitTextField` (ui.tsx).
