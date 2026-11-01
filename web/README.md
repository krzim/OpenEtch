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
- **Text and barcodes** (`src/text/`, LightBurn's Text tool, Text Options, Variable Text and Create Bar Code; engine
  commands in ARCHITECTURE.md "Text"). Text tool (T, LightBurn's Ctrl+T where the browser passes it; toolbar): click
  places a text box at the (snapped) point; Enter starts a line, Esc / Ctrl+Enter / a click elsewhere finishes
  (`shape.text`; an empty box adds nothing). Clicking a text with the tool, or double-clicking it with Select, edits
  it in place (`text.update`; emptied text is deleted). While the tool is active the options bar sets font, height,
  HSpace/VSpace, Align X/Y, bold/italic/upper/welded for new text (kept in this browser) and for the text being edited
  or selected. Properties of a text: the text or format, a searchable font picker (`font.list`: installed, Hershey
  single-line, SHX, JSF), every setting above plus bend and distort, max width and squeeze, text on a path (pick the
  shape, offset, follow again, detach), Text Mode (Normal, Date/Time, Serial Number, Merge CSV, Cut Setting) with the
  offset and LightBurn's code table, ignore empty vars, refresh and convert to path. A text whose font the engine lacks
  gets a "Missing font" badge on the canvas and in Properties. Apply Path to Text also works on a text + shape
  selection and from the context menu. The Variable Text window (docked under Properties while the document uses
  variable text, or from the context menu) edits Start/End/Current/Advance by/Auto-advance (`var.set_serial`),
  Previous/Next/Reset (`var.advance`), Test (`var.preview_row`, changes nothing) and the CSV file (`var.load_csv`,
  `var.clear_csv`). Barcode tool: click to place, the Create Bar Code dialog sets type, data, module/bar size, bar
  height, invert and quiet zone, error correction, force square, show text and variable data (`shape.barcode`);
  double-click a barcode, or use its Properties section, to redraw it in place (`shape.barcode {id}`).
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
- **Laser window and machine** (LightBurn's Laser window, Live Framing, Preview, Device Settings, Calibrate Galvo Lens,
  Rotary Setup and Optimization Settings; `workspace/LaserDock.tsx`, `components/JobControls.tsx`,
  `FramingOptions.tsx`, `PreviewTimeline.tsx`, `DeviceEditor.tsx`, `LensWizard.tsx`, `RotaryDialog.tsx`,
  `OptimizationDialog.tsx`, `CutOptions.tsx`; pure logic in `api/job.ts`, `api/deviceProfile.ts`, `api/cutOptions.ts`,
  HTTP calls in `api/jobApi.ts`):
  - **Job**: progress bar, state, run n of N, part, elapsed and remaining time from `GET /api/job`, kept current by
    the `job_progress` events (useEngine publishes every event on `api/events.ts`; the hook refetches on the machine
    events and polls every 4 s while a job is active). Pause (never gated), Resume (UI only) and "End after this run"
    for repeating jobs; the Pause / resume hotkey (unbound by default). Count, Reset Count and Stop At (empty = off).
  - **Run options** for Request run (dock, toolbar and hotkey): Repeat (1-10000), Run continuously, Wait for foot pedal
    (Start Marking input) with Require Full Pulse, Cut Selected Graphics; kept in this browser (`components/jobStore.ts`,
    Cut Selected Graphics never restored). Every request is still a pending run that needs the PIN.
  - **Framing**: Frame with an options popover: Bounds / Hull / Contour, Frame individually, Outside shapes only,
    Selection only, Tool layers only, once or continuous, frame speed, red dot / focus light / both, blanking (device
    default, on, off) and Live framing. While live framing runs, arrows move the framed shapes 5 mm (Shift 20, Ctrl 0.5),
    Page Up/Down scale and `.` / `,` rotate 15° (Shift 1.5°, Ctrl 45°), one `object.transform` each, on the selection or
    every output shape. The framing hotkeys use the same options.
  - **Preview timeline** (replaces the segment slider): a slider over the job's estimated time from the segments' `t_s`,
    play / pause at 0.25x-100x real time, scrubbing sets the Viewport's `upTo` (the WebGL toolpath is drawn up to that
    time; `toolpathGL.ts` unchanged), "Start here" (`POST /api/runs {from_segment}`, the first segment not shown) and
    Save preview image (`GET /api/preview.png?width=3200`).
  - **Device settings** (Devices window > Edit…, also Duplicate and Import EzCad… for a markcfg): pages Galvo and basic
    (name, field size, offset, angle, red dot offset / scale / rest, galvo 1 is X, reverse X/Y, frame / max / min speed,
    galvo 1 and galvo 2 scale / bulge / skew / trapezoid mapped to X or Y like LightBurn, jump and delay defaults, the
    jump-speed and delay limits), Ports and laser (laser and fiber type, Q-pulse width, open MO delay, frequency range,
    power limit, start / stop marking inputs, focus light output, blanking; the red-dot and laser-enable bits read only;
    inputs are active high), Z axis / rotary (the extension axis), Lens (load / clear the COR file, the lens wizard,
    the current corrections) and Controller (`GET /api/machine/info`). Values come from `GET /api/devices/{id}`
    `profile`, the device's `.lbdev` export and, for the active device, the controller info; a value none of them
    reports is shown as "not reported" and only sent when typed. Save sends a JSON merge patch of the edited fields to
    `PUT /api/devices/{id}`; client checks mirror the engine's (limits only go down) and an engine refusal is shown at
    the field it names (`errorField`). The built-in device is read-only ("Duplicate to edit"); Export .lbdev downloads it.
  - **Calibrate galvo lens**: add the test pattern (`POST /api/calibration/pattern`), request a run of only it (Cut
    Selected Graphics; PIN approval in the Runs card), enter the eight measured lengths (lettered on a diagram), solve
    and compare current and new corrections, Apply to device (`apply: true`, UI only; 409 on the built-in device).
  - **Rotary setup** (`job.set_rotary`): enable, chuck / roller, steps per rotation, object and roller diameter, split
    size, cylinder correction, taper; circumference, steps per turn, strips for the design, a warning when the design
    is taller than the circumference or the active device has no rotary axis, and Test rotate (to the second strip and
    back, `POST /api/machine/axis`). The engine has no strip overlap.
  - **Optimization settings** (`job.set_optimization`, the changed keys only): enable, Order By (layer / group /
    priority, reorderable), inner first, direction order, reduce travel, reduce direction changes, best start and
    corners, best direction, remove overlaps with the distance, sort cuts last.
  - **Cut settings** (Properties > a layer's pass > More settings): global passes, the pass's own output toggle, lines
    per inch, fill grouping, flood fill, auto-rotate, dot width adjust, dot mode, wobble, perforation, tabs, pause
    before / after, Z offset / step, and Override timings (prefilled from the active device's timing). Shape
    properties for a selection: Power scale and cut priority (`object.set_cut_properties`).
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
- **Files, projects and image tools** (`src/files/`, one hook `useFilesUi` for App.tsx; engine side in ARCHITECTURE.md
  "Files and image tools", routes in crates/engine/src/userfiles.rs; pure logic in `files/fileFormats.ts`, tested;
  HTTP in `files/filesApi.ts`; strings in `workspace/locales/en-files.ts`; styles in `files/files.css`):
  - **Export** (File > Export…, Alt+X): SVG, DXF (keep circles, tolerance), AI (PDF-compatible) or PDF of the
    selection or the whole project, layer colours or black, design bounds or workspace (`GET /api/export`). **Print**:
    black only / keep colours, line width, selection only; Print opens the browser's print dialog on the print SVG at
    1:1 in a hidden frame, Save PDF downloads the PDF fitted to A4/A3/Letter/Legal (`GET /api/print`). **Export
    image** and **Save processed bitmap** for a selected image (File menu and context menu).
  - **Imports of every further format**: PDF, AI, EPS, HPGL/PLT, Gerber/Excellon, STL/OBJ/OFF/PLY/3MF/AMF, TIFF/TGA,
    EzCad `.ezd`, legacy `.lbrn` and `.lbt` go to `POST /api/document/import` with the raw file (100 MB). Picked
    (Ctrl+I) or dropped files reach `intercept` before App's router (on engines with `art.import_pdf` it also takes
    SVG, DXF and bitmaps, so the import options apply to all); 3D models open the Import 3D model dialog (rotation,
    width, DPI, depth cutoff, 16-bit, invert, transparent background, equalize, outline) and Gerber files the Gerber
    dialog (type, invert); File > Open > Upload opens any of them as a new project (`mode=replace`, after the
    unsaved-changes prompt) and a `.lbt` opens as a new untitled document. **Import options** (File menu and the
    import dialog, kept in this browser): group, select and centre imported shapes, import hidden AI/PDF layers.
  - **Projects**: Notes window (Ctrl+Alt+N, `doc.set_notes`, "Show notes when the file opens": shown once when a
    file is opened, detected by the fresh one-step timeline an open starts); Restore from autosave / backup (lists,
    restore as an unsaved document, autosave interval and how many are kept) with the autosave status in the File
    menu; templates (Open template with Upload .lbt, Save as template, Recent templates kept in this browser);
    Preferences and bundles (export / import `.oeprefs`, the automatic backups, user bundles of preferences, devices
    and libraries; imports are confirmed first); project thumbnails in the Open dialog.
  - **Pages** dock (Design panel): switch, add, duplicate, delete, rename (double-click), reorder (drag or Move up /
    down); the canvas context menu moves the selection to another page.
  - **Image tools** (context menu, hotkeys): Trace image (Alt+T: live preview over the image, Fade image, Show points,
    boundary dragged in the preview, Cutoff / Threshold / Ignore less than / Smoothness / Optimize, transparency,
    centre line, sketch, delete image after), Adjust image (Alt+I), Convert to bitmap, Apply / Remove / Flatten mask
    and Crop to mask (the canvas clips a masked image to its mask), Replace image / to fit / Refresh (pick the edited
    file). The Image section's **3D Sliced** dither shows Slices (the layer's passes), Enable cleanup pass, Clean
    after # of passes and Clean whole image (`files/Slice3d.tsx`).
- **Libraries, defaults, laser tools and job safety** (`src/libs/`, one hook `useLibsUi` for App.tsx; routes in
  crates/engine/src/api_libs.rs; pure logic in `libs/model.ts` and `libs/testgen.ts`, tested in `libs/model.test.ts`;
  HTTP in `libs/api.ts`; shared state in `libs/store.ts`; strings in `workspace/locales/en-libs.ts`; styles in
  `libs/libs.css`):
  - **Material Library** dock (both workspaces; File > Material Library, layer menu > Library…): Select Library with
    Load (.clb) and Manage Library (Save, Save As, Create New, Merge With, Rename, Unload with save / discard,
    Download .clb), a searchable tree Material > Thickness or Title > Description, the target layer (the selected
    one), Assign, Link, Edit Cut, Update, Edit Description, Duplicate, Delete (also on the entry's right-click menu)
    and Create new from layer. A linked layer shows a link badge in Cuts / Layers and the Library tree, its cut
    settings are read-only (Properties shows Unlink; the quick-edit row is disabled), and Edit Cut starts from a linked
    layer's full settings when there is one (else only the parameters shown are kept).
  - **Defaults**: Properties > Defaults and the Cuts / Layers menu: Make Default, Make Default for All, Reset to Default,
    Reset to OpenEtch defaults, Load default layer settings on New / restart. Optimization Settings: Set as Defaults,
    Reset to saved, Reset to OpenEtch defaults, Forget saved defaults.
  - **Laser Tools** (toolbar dropdown and Laser window): Material Test, Interval Test and Focus Test dialogs with
    LightBurn's options and a live preview numbered in LightBurn's safe burn order (Create, or Create and request run);
    Set start point (click a node of a closed shape; Shift reverses, Ctrl clears, Esc ends; start points are marked
    on the canvas; also on the canvas menu); Print and Cut (two targets from the selection, find each with the red dot,
    Align with optional scale); Camera (shown only when the engine has `/api/camera`: capture from the engine's UVC
    camera, this browser's camera or an image; lens calibration from checkerboard views; alignment from four burned
    marks; the camera image on the canvas with an opacity slider).
  - **Laser window**: Red dot always on (the device's IO setting), Batch (saved projects in order, one approval), Repeat
    table (Repeat Marking with a linear or rotating table axis), Job log (newest runs, totals, Save as CSV). Framing
    options gain Image bounds; Cut settings gain Offset Fill and Kerf offset.
  - **Approval gates**: the confirm step of a pending run lists Require Framing and Door Protect of the active device
    and the Job Checklist, whose every item must be ticked before Confirm and fire (sent as `checklist`); the engine's
    refusals get specific texts. A "SIMULATOR TEST PIN" badge shows in the top bar and the Runs card while
    `approval.sim_pin` is true. Device Settings > Job edits Require Framing, the checklist items, Play sound (plus a
    per-browser mute and Test sound) and Load defaults on New; Device Settings > Ports has LightBurn's IO ports (Door
    Protect and Start Framing inputs with their level, Busy / Ready / Done Marking outputs and pulse, Red Dot Always On);
    the extension axis can be a linear or rotating table. The `job_finished_sound` event plays a short chime.
  - **File and Help**: Find My Laser / Add galvo (USB scan, markcfg7 + .cor, name, make active), Batch run, Job log;
    Help > Enable debug log, Report a bug (recent log, Download bug report), Check for updates (opt-in).
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
  api/job.ts, api/deviceProfile.ts, api/cutOptions.ts   job status and run/frame bodies, timeline maths; the device
                      settings model and merge patch; cut options, optimization, rotary, shape properties (pure; tested)
  api/jobApi.ts, api/events.ts   job, framing, calibration and device-setting routes; the engine event bus
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
  libs/               Library dock, defaults, test generators, approval gates, laser tools, camera, Find My Laser,
                      Help additions (useLibsUi.tsx for App; model.ts and testgen.ts are pure and tested; libs.css)
  text/               Text and Barcode tools, text/barcode properties, Variable Text window, font picker
                      (useTextTools.tsx wraps EditTools' canvas tool; textModel.ts is pure and tested; text.css)
  files/              Export / Print, imports of further formats, templates, notes, backups, prefs and bundles, Pages
                      dock, Trace Image, Convert to Bitmap, masks, 3D Sliced (useFilesUi.tsx; fileFormats.ts pure, tested)
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
