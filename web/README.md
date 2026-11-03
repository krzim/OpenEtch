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

## User guide

The user guide's pages are markdown in `../docs/guide/` (the source of truth, also read on GitHub; the page format is
in `docs/guide/README.md`). `src/guide/` shows them in the app: `markdown.ts` parses them (no dependency, no HTML:
render.tsx builds React elements), `pages.ts` holds the contents, previous / next and search, `route.ts` the
`#/guide/<page>#<anchor>` routes, and `GuideHost.tsx` shows the viewer beside the canvas (desktop, MacBook, iPad
landscape) or as a full-screen sheet (phone, iPad portrait). The viewer and the pages are one lazy chunk
(`GuideViewer.tsx`, ~12 KB gzip plus the pictures, which load only when shown), so start-up does not grow by them;
the main bundle carries only the route, the frame and the strings (~3 KB gzip). Help > User guide, `F1` and command
search open it; `?guide` in the address shows the guide alone (Open in new tab). `VITE_OPENETCH_DESIGN_ONLY=1` leaves
out the pages with `needs: machine` (the future design-only build).

To add a page: write `docs/guide/<slug>.md` with its front matter (headings get GitHub's ids from their text, see
`docs/guide/README.md`; do not add `{#...}`), run `npm run guide:index` (the contents list at the end of index.md)
and `npm test`. To add a screenshot: list it in `docs/guide/shots.json` (the step language is in `e2e/README.md`,
"Guide screenshots"), reference it as `![Alt](img/<name>.webp)` and run `npm run guide:shots`. To link a panel to the
guide: `<HelpLink to="page#heading-id" topic={t("...")} />` (`src/guide/HelpLink.tsx`); `src/guide/helpLinks.test.ts`
fails when the page or the heading does not exist.

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
  Polygon/star (drag from the centre; sides, star ratio and corner radius in the options bar; on engines with
  `shape.update` it stays a parametric polygon, drawn by `geometry.ts` `polygonCmds` exactly as the engine outlines it,
  and Shape Properties edits it), Rectangle/Ellipse Ctrl
  from the centre and Shift square. Edit nodes (Ctrl+`) lists the shape's nodes with `node.list`: drag nodes, handles
  or segments (live preview, `node.move` on release), click a segment to add a node, marquee/Shift selects; keys at
  the pointer or on the selection: Delete/D, B break, J join, S smooth, C corner, L line, K curve, I/M insert, T trim,
  E extend (the open end nearest the pointer straight on to the nearest line, `node.extend`);
  rectangles and ellipses offer Convert to path first. Measure (distance, ΔX, ΔY, angle, snapped), Trim (click a
  piece). Modify and Arrange dropdowns: weld (Ctrl+W), union/subtract/intersect (Alt++ / Alt+- / Alt+*), XOR, cut
  shapes (Alt+C), offset (Alt+O), fillet, break apart (Alt+B), auto-join (Alt+J), close, delete duplicates (Alt+D),
  convert (Ctrl+Shift+C), optimize (Alt+Shift+O: fit to lines, fit to arcs and smoothing, each with its tolerance),
  reverse; draw order (PgUp/PgDn, Ctrl+PgUp/PgDn), lock, make same size, Transform (scale %, shear, mirror across a
  picked line, also keeping the original as LightBurn's copy option, two-point rotate Ctrl+2 / scale), grid and
  circular arrays (centre pick), copy along path, nest (true shape or Quick Nest boxes, into the stock, the field or the
  shape selected last; parts that did not fit are reported). Rulers (in the display unit) run along the top and left;
  drag a guide out of a ruler (back onto it to remove; guides move with the Select tool; double-click one, or Arrange >
  Add guide, to type its position). Guides are saved in the document (`guide.add` / `guide.move` / `guide.delete`);
  guides an older version kept in this browser for the project move into the document once. Engines without guide
  commands keep them per project in this browser as before (`src/guides.ts`). Guides are snap targets. Scaling with
  the selection handles snaps the dragged handle to other objects and shows the alignment guides (an edge handle only
  along its axis). The Viewport hands presses to these tools through `src/canvasTool.ts`.
- **Design tools** (`src/design/`, one hook `useDesignUi` for App.tsx; commands in crates/commands/src/builtin/
  design.rs, construct.rs, art_library.rs; pure logic in `design/model.ts`, tested in `design/design.test.ts`; HTTP
  and command builders in `design/designApi.ts`; styles in `design/design.css`):
  - **Edit** dropdown: Cut, Copy, Paste (centred on the pointer), Paste in place (Alt+V), Use system clipboard (on by
    default, as in LightBurn; kept in this browser), Select open shapes, Select open shapes set to fill, Select
    contained shapes (`select.*` queries), Curve tolerance (`doc.set_curve_tolerance`, saved with the project; empty
    uses the device's).
  - **Clipboard**: the internal copy is the selection as an Art Library fragment (objects, groups, layer names and
    colours), so pasting after Cut brings back images, groups, text and parametric shapes; a paste is one batch of
    `art.insert` per top-level piece (one undo step; layers matched by colour). With the system clipboard on, Copy also
    writes the selection as SVG in mm (y flipped, layer colours, images embedded) as text and, where the browser
    allows, `image/svg+xml`, tagged with a marker; Paste reads the page's paste event (no permission prompt, works on
    a LAN address too): our own copy comes from the internal one, SVG code (Inkscape, Illustrator's "Include SVG
    Code") is imported, a bitmap is imported as an image, other text becomes a text object, each centred on the
    pointer. The menu's Paste uses the async Clipboard API when the browser allows it, else the internal copy.
  - **Tools** dropdown: Warp selection (four corner handles, Perspective or Envelope) and Deform selection (4 x 4
    handles): drag handles over a live preview (the engine's maps, `model.ts` `warpMapper`), Shift keeps horizontal and
    Alt vertical symmetry, double-click resets a handle, Enter or Apply sends one `path.warp`, Esc cancels. Generate
    tangent circle (three circles selected: the candidates in light red, click one or Create all), Rubber-band outline,
    Convert toolpath to path (the selection or every output shape), Art Library.
  - **Arrange** additions: Move selected objects (the nine page positions of the workspace or the stock with a margin,
    `object.move_to_page_position`), Move horizontally / vertically together (fixed spacing, `object.distribute`
    by spacing). **Grid array** (Arrange and the toolbar): columns, rows, padding between edges or centre to centre,
    reverse X / Y, alternate row / column shift (mm or half), mirror alternate columns / rows, with a live preview of
    the copies in the workspace and a warning for cells outside it.
  - **Shape Properties** (Properties, a lone rectangle, ellipse or polygon): width, height, corner radius; radii;
    sides / points, radius, star and inner radius, corner radius (`shape.update`, the centre and rotation stay).
  - **Art Library** dock (Tools > Art Library, the Window menu): libraries the engine keeps in its projects folder
    (New, Load .oeart, Download, Rename, Delete), thumbnails (searchable), Add selection, Import a file, double-click to
    add at the field centre or drag onto the canvas (centred on the drop point), right-click to rename, delete, copy or
    move an item to another library; follows `art_library_changed`. LightBurn's `.lbart` is refused with the reason.
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
- **GRBL gantry windows** (only while the active device is a GRBL gantry, `GET /api/machine` `driver: "grbl"`;
  `components/GrblMove.tsx`, `GrblConsole.tsx`, pure logic in `api/grbl.ts`; unverified on hardware, tested against
  the engine's GRBL simulator): **Move** (LightBurn's Move window: position from `GET /api/machine/grbl?refresh=true`
  polled every second, jog buttons with distance and speed, arrow keys while the window has the focus (one jog per
  press, auto-repeat ignored; Shift x10, Alt /10, Esc stops the jog; arrows never reach the canvas from there), Home, Unlock, Go to origin, Move to a machine position; every move is a laser-off jog),
  **Console** (lines to `POST /api/machine/console`, the board's answers, Up/Down history, Status, and macros saved
  with the device through `PUT /api/prefs` `console_macros`; the engine refuses anything that could turn the laser
  on, typed or from a macro), **File > Save G-code** (`GET /api/export/gcode`), and the GRBL layer options in the
  pass's More settings (constant power M3, air assist M8, overscan, overcut).
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
  api/grbl.ts         a GRBL gantry's Move / Console windows and Save G-code: status, coordinates, jogs, console
                      log and history, macros (pure; tested)
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
  design/             Shape Properties, warp / deform, tangent circle, rubber band, toolpath to path, selection
                      queries, page positions, grid array, curve tolerance, Art Library dock, clipboard (useDesignUi.tsx;
                      model.ts pure and tested; design.css)
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
- **Document tabs** (`DocTabs.tsx`, LightBurn's File > New Window on the engine's open documents, `GET/POST
  /api/documents`): one tab per open document with an unsaved dot; a click activates it (every edit, save, preview and
  run then goes to it, and the selection is dropped); "+" makes a new document or opens a saved project in a new tab
  (also File > New tab and the unbound "New document tab" hotkey); × closes a tab and asks first when it has unsaved
  changes (Discard and close, Show it, Cancel); middle-click closes too. The strip follows `documents_changed`, so a
  switch made by an agent or another browser shows. Only the active document is autosaved (engine).
- **Dockable windows** (`layout.ts`, `Dock.tsx`, `DockHost.tsx`, `windows.ts`; LightBurn's docking and Window menu):
  every tool window (`<Dock id>`: Browser, Laser, Pages, Cuts / Layers, Material Library, Art Library, Variable Text,
  Docking, Shape Properties, Machine and runs) can be docked left, right or in the bottom strip under the canvas,
  floated (drag its title, resize from the corner), tabbed with another window, moved up or down and hidden, from the
  ⋯ button or a right-click on its title. A Dock portals its frame into the slot its area's host renders, so the
  components that render it are unchanged (without hosts, e.g. in tests, it renders in place). The top-bar Window menu
  shows or hides each window (windows of the other workspace are marked) and has Reset to default layout. The layout
  is kept per browser (`openetch.layout`, sanitised on load); an empty side panel is hidden.
- **Docking tool** (`docking.ts`, `DockingTool.tsx`; LightBurn's Arrange > Dock and Docking toolbar): Dock left /
  right / up / down slide the selection until its outline touches another visible shape's outline or the workspace
  edge (rays cast from every vertex both ways, so shapes dock snugly). Shapes inside a container are not stopped by
  its inner edges; shapes overlapping at the start are no obstacle. Without Move as group each unit docks on its own,
  nearest the destination first, so a row stacks up; Lock inner objects carries selected inner shapes with their
  container; Padding keeps a gap in the direction of travel. One batch of `object.transform`, one undo step. In the
  Docking window, the canvas menu and as unbound hotkeys.
- **UI scale** (Preferences > Appearance): text size (`--ui-scale`), icon and toolbar size (`--icon-scale`: every
  `Icon` and the toolbar buttons) and handle and node size (`--handle-scale`: selection and rotation handles, node
  squares and handles, and how near the pointer must be to pick a node), LightBurn's font, toolbar icon and
  larger-cursor/node settings.
- **Languages** (`i18n.ts`, `locales/`): every UI string is a message key; `en-ui.ts` holds the older components'
  strings (Properties, File menu, Toolbar, Edit tools, top bar, dialogs, Devices, Laser panel...). `locales/es.ts`
  is a machine-translated Spanish skeleton (marked "traducción automática" in the selector) that proves the
  framework; untranslated keys fall back to English. Keys built at runtime (shape kinds, Edit tools verbs) go
  through `useTD()` with the English text as fallback.
- **Cuts / Layers extras**: Flash layer (right-click menu, or Shift+right-click a row) blinks the layer's shapes on the
  canvas (`flash.tsx`); T1 / T2 tool layers (`Layer.tool`, `layer.tool`) read "Tool", never output and can be made or
  cleared from the layer menu, and the palette's T1 / T2 swatches put the selection on the real tool layer; a layer
  with LightBurn settings OpenEtch will not execute shows a "!" badge listing them (`GET /api/layers/unsupported`,
  asked only for documents with `lbrn2.*` layer settings); a layer with sub-layers shows a tab per pass above the
  quick-edit fields (speed, power, passes, frequency, Q-Pulse, interval of that pass; drag a sub-layer tab to change
  the order, `layer.sub_move`).
- **Hotkeys (LightBurn 2.0 cheat sheet)**: besides the earlier defaults, Ctrl+P / Ctrl+Shift+P print (black only /
  keep colours), Ctrl+Shift+B convert to bitmap, Ctrl+K trim, Alt+M measure, Ctrl+Shift+M mirror across line,
  Alt+Shift+C cut shapes (Alt+C too), Alt+Home align centres, Alt+Shift+H / V move together, Alt+S request run,
  Pause pause/resume, Alt+L position laser (the next canvas click points the red dot there), Ctrl+Shift+R rotary
  setup, Alt+0 Print and Cut. Not mapped because the browser keeps them or a galvo has no such thing: Ctrl+Tab (add
  tabs), Ctrl+Q (exit), F12 (toggle side panels: bindable, unbound), Alt+Shift+L (save G-code), the jog keys, Ctrl+H
  (home), Alt+Shift+S (send to laser), Ctrl+Shift+F (feeder) and Alt+1..4 (Print and Cut steps inside its dialog).
- **Also**: Calibrate Galvo Lens takes the two optional corner shifts (I: top-left in X, J: bottom-right in Y, against
  a straight edge) that separate the X and Y skew (`top_shift_mm`, `right_shift_mm`); Device Settings has Field width
  and Field height (empty = square) and the canvas draws a non-square galvo field from the device's workspace; Import
  options has "Allow importing to tool layers" (`tool_layers=true` on `/api/document/import`; off, imports never land
  on T1 / T2); the camera overlay is drawn under the design (`CanvasTool.underlay`); Enter in a dialog field commits
  the field before the dialog submits (Modal blurs, then submits on the next tick).
- **Display units everywhere**: besides fields, the rulers, canvas readouts (cursor, field size, drag/size, node and
  Measure readouts), cut-settings summaries, Laser stats, hints and dialog messages use the length and speed units;
  dialogs share `UnitTextField` (ui.tsx).
