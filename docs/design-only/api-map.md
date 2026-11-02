The browser can already run almost all of the document, file, image and text work: the Rust code behind those endpoints compiles to wasm32. What must be rebuilt for a design-only site is the engine's own glue around that code. That means session and project state, the fill-in of field sizes and layer defaults, storage (IndexedDB instead of disk), events, and stub machine answers. The real wasm blockers are few: DXF import, PDF and AI import, installed fonts, the clock, and PNG rendering (resvg).

**How I checked:** I ran `cargo check --target wasm32-unknown-unknown -p commands -p machine -p corfile -p lmc` with a scratch target dir (`--offline -j 2`). It finished in 1m03s with no warnings or errors. `commands` pulls in doc, lbrn, geom, fileio, plan and text (without the `system` feature). `machine` also compiles for wasm with default features, which matters because the preview's `Target` and the device layout depend on it. Nothing in the repo was edited.

## How the web app reaches the engine (main @ d9129a1)
- There are six `fetch` wrappers: `web/src/api/client.ts` (`send`), `web/src/api/jobApi.ts`, `web/src/design/designApi.ts`, `web/src/files/filesApi.ts`, `web/src/libs/api.ts` and `web/src/workspace/wsApi.ts`.
- There is one WebSocket, in `web/src/api/useEngine.ts`, using `eventsUrl()` → `/api/events`.
- No `<img src="/api…">` or `window.open` points at an engine path. Images are drawn from data URIs built from the document JSON (`SceneCanvas.originalHref`). So one fetch-level adapter, or a service worker, can catch every call.
- All paths are absolute `/api/...`. That is fine for a shim, but Vite needs a `base` setting for the assets under `github.io/<repo>/`.
- `useEngine` polls `GET /api/machine` every 3–10 s, `JobControls` polls `GET /api/job` every 4 s, and `api/commands.ts` checks command ids against `GET /api/commands` at startup.

## Endpoint map
WASM = every crate function the handler reaches compiles for wasm32. GLUE = the logic sits in the engine crate (axum, tokio, std::fs) and must be re-created or moved into a shared crate.

| Group | Endpoint (web caller) | Engine handler → crate functions | wasm32 |
|---|---|---|---|
| Doc/commands | GET /api/commands (client, commands.ts) | `AppState::specs` → `commands::Registry::specs` | WASM. `art.import_dxf` and `art.import_pdf` are not registered on wasm, so the startup check will report `importDxf` missing |
| Doc/commands | POST /api/execute, /api/batch (client, drop, useDesignUi) | `AppState::batch_opening` → `commands::Session::batch` | Session: WASM. GLUE: Query/Edit risk gate, `libs::fill_defaults` (layer and optimization defaults from prefs), field/workspace fill for `doc.new` and `doc.import_lbrn2` (`new_layout` / `lightburn_layout` from `machine::Device`, `commands::lbrn2_device_name`, `LightBurn::field_for_file_device`), `replaces()` → ProjectState, `field_source` annotation, events |
| Doc/commands | POST /api/undo, /api/redo, /api/history/goto, /api/history/delete; GET /api/history (client, history.ts, Timeline) | `Session::undo/redo/goto/delete_step/history_view` through `timeline_op` | WASM, plus GLUE for events |
| Doc/commands | GET /api/document | `Session::document` | WASM |
| Doc/commands | GET/POST /api/documents, /documents/{id}/activate, /documents/{id}/close (wsApi, DocTabs) | `documents.rs` DocSet of Sessions, `Session::raise_revision` | WASM core. GLUE: DocSet, the cross-document revision high-water mark, opening a project from disk |
| Doc/commands | GET /api/layers/unsupported (CutsLayers) | `lbrn::unexecuted_settings` | WASM |
| Queries | GET /api/preview[?layers] (useEngine) | `plan::plan_job_with`, `plan::preview_document`, `plan::scale_times`, `Preview::retain_layers`, `lbrn::unexecuted_settings` | WASM. GLUE: Target from `hw::target_from_profile` (machine crate, compiles), job-counter `run_index`, marktime correction, PreviewGate coalescing (tokio). A not-controllable device returns `PlanError::Target` today, so you need to decide how design-only previews behave |
| Queries | GET /api/preview.png (PreviewTimeline, jobApi) | `render::preview_svg` (engine, pure over doc/plan) + `render::svg_to_png` (resvg) | GLUE. resvg is not checked for wasm; drawing the SVG to a canvas is the alternative |
| Queries | Stats | Inside the `/api/preview` answer (`Preview.stats`) and the `doc.inspect` command; there is no separate route | WASM |
| Queries | GET /api/project (useEngine, project.ts) | ProjectState: name, display_name, saved/clean revision, dirty | GLUE |
| Queries | Not used by the web app | /api/fill_regions (`plan::fill_regions`), /api/preview.svg | WASM |
| Files | GET /api/document/download?format=openetch\|lbrn2 (client) | `saveable` (doc::file serialization plus reload check); `doc.export_lbrn2` (lbrn writer) | WASM. GLUE: filename stem from the project |
| Files | POST /api/document/upload?format=openetch\|lbrn2\|svg\|dxf\|image (client, drop, project.ts) | `AppState::upload` → `doc.load`, `doc.import_lbrn2`, `art.import_svg` (usvg), `art.import_dxf`, `art.import_image`; `sniff_image` | WASM except dxf. dxf → uuid → getrandom is native-only (see blockers) |
| Files | POST /api/document/import?format=… (filesApi, fileFormats, drop) | `userfiles::import_invocation` and `import_layer` (tool layers; replace = `doc.new` + import in one batch) → `art.import_svg/hpgl/gcode/gerber/image/eps/ezd/model`, `doc.import_lbrn2` (lbrn, lbrn2, lbt) | Those are WASM. Not WASM: `art.import_dxf`, `art.import_pdf`, `art.import_ai` (lopdf, wasm off; a PDF-based AI fails on wasm). GLUE: the routing logic |
| Files | GET /api/export?format=svg\|dxf\|ai\|pdf | `doc.export` → fileio svg_out, dxf writer, pdf_out | WASM |
| Files | GET /api/print | `doc.print` → fileio pdf_out / svg | WASM |
| Files | GET /api/export/gcode (grbl.ts, jobApi) | `commands::prepare_output`, `plan::plan_job_with`, `plan::gcode_lines/gcode_file` with a GRBL profile | WASM, but needs a machine/GRBL profile. Optional for design-only |
| Files | GET /api/images/{id}/file, /api/images/{id}/processed.png (filesApi) | `image.export`, `image.processed` (plan dither) | WASM |
| Files | GET /api/projects, POST /api/projects/save, POST /api/projects/open; /api/projects/folders, /api/projects/locations (client, filesApi, folders) | ProjectDir (std::fs), backup on save, `doc.load` | GLUE: fs → IndexedDB or download |
| Files | GET /api/projects/{name}/thumbnail.png (ProjectDialogs) | `doc::file::from_openetch_json` + `render::preview_svg` + `svg_to_png` | GLUE: resvg or canvas |
| Files | Autosave and backups: GET /api/backups, POST /api/backups/restore, GET/PUT /api/files/settings | `spawn_autosave` (tokio timer), rotation, `doc.load` | GLUE |
| Files | Templates: GET /api/templates, POST /templates/save, /templates/upload, /templates/open | `fileio::lbrn_legacy::to_lbrn2`, `lbrn::parse`, `doc.load` / `doc.import_lbrn2` | WASM core, GLUE storage |
| Files | Art libraries: /api/art/libraries/* (designApi, ArtLibrary) | ArtStore (fs), `commands::art_fragment`, `fileio::lbart::read/write`, `lbrn::shapes_to_document/shapes_from_document`, `art.insert`, import through a scratch Session, thumbnails via `svg_to_png` | WASM core. GLUE: storage, resvg thumbnails, `art_library_changed` |
| Files | Material libraries: /api/library/* (libs/api) | `lbrn::parse_clb/write_clb/cut_setting_from_layer`, `layer.apply_preset` | WASM core, GLUE storage |
| Files | Prefs: /api/prefs, /api/layers/{id}/default and /reset, /api/optimization/default and /reset, /api/prefs/export, /api/prefs/import, /api/prefs/import_lightburn, /api/prefs/backups, /api/prefs/backups/restore | PrefStore (fs JSON), `lbprefs.rs` | GLUE |
| Files | Bundles: /api/bundle/export, /api/bundle/import (.oebundle, .lbzip) | `fileio::zip`, `lbzip.rs` | WASM zip, GLUE. `.lbzip` is out of scope |
| Files | Presets: /api/material-test/presets*, /api/image-adjust/presets* | `commands::testgen_presets` (lbmt) | WASM core, GLUE storage |
| Files | LightBurn devices: GET /api/lightburn, POST and DELETE /api/lightburn/prefs | `machine::parse_devices/find_device/profile_from_lightburn` (compiles); reads prefs.ini from disk | Upload path: WASM. Disk read: native only |
| Images/trace | All through /api/execute: `image.adjust/trace/trace_preview/apply_mask/remove_mask/flatten_mask/replace/edit_model/processed/export`, `art.convert_to_bitmap`, `art.import_image` (ImageSettings, ImageTools, Slice3d) | `fileio::trace`, `fileio::raster`, `fileio::mesh`, plan dither, the `image` crate (png/jpeg/bmp/gif/webp/tiff/tga, pure Rust) | WASM. Costs are speed and bundle size, plus base64 image data in every document JSON passed across a worker |
| Fonts/text | All through /api/execute: `font.list`, `shape.text`, `text.update/apply_path/detach_path/convert_to_path/refresh`, `var.*`, `shape.barcode` (FontPicker, textModel, Barcode, VarTextWindow) | text crate: skrifa, harfrust, hershey/shx/jsf, rxing encoders | WASM without `system`: only the bundled Hershey fonts, no installed fonts, no font folders, no clock. There is no font HTTP endpoint |
| Machine/frames/runs/approval | /api/machine (polled), /machine/frame, /frame/stop, /stop, /pause, /resume, /pointer, /axis, /info, /inputs, /grbl, /home, /unlock, /jog, /jog/cancel, /console, /ports; /api/runs, /runs/{id}/approve, /job/start, /job (polled), /job/settings, /job/count/reset, /job/end_after_current; /calibration/pattern, /calibration/solve; /sim/inputs | hw, runs, approve, pin, job, frame (tokio, nusb, serial2) | Drop. Stub `GET /api/machine` and `GET /api/job` with a "no machine" answer, and hide the controls |
| Devices | /api/devices, /devices/{id} (GET, PUT, DELETE), /activate, /duplicate, /lbdev, /cor, /devices/import, /devices/import/markcfg, /devices/grbl, /devices/scan, /devices/wizard | DeviceRegistry (fs TOML), machine crate | The machine crate compiles. Needed: at least one design device for field and workspace (CutOptions, DeviceEditor, `doc.new` layout). Scan, wizard and cor: drop |
| Camera and other | /api/camera* (11 routes), /api/job-log, /api/diagnostics/report, /api/diagnostics/logs, /api/updates, /api/updates/check | camera, nokhwa, joblog, updates | Drop |
| MCP | /mcp (rmcp streamable HTTP: `list_commands`, `execute`, `batch`, `inspect_document`, `render_preview`, `undo`, `redo`, `machine_status`, `project_*`, `documents_*`, `history_*`, `frame_*`, `job_request_run`, …). The web app only shows the URL (HelpMenu.tsx:158) | mcp.rs | Not possible on a static host; drop it and hide the Help entry |

**Events (WS /api/events)** carry these message types: `document_changed`, `history_changed`, `project_saved`, `documents_changed`, `art_library_changed`, `device_changed`, `devices_changed`, `machine`, `run_pending`, `run_finished`, `approval_locked`, `job_progress`, plus the lag message. An in-browser backend has to emit the first five into `publishEngineEvent` and the `useEngine` switch. The rest never fire.

## What blocks wasm, and likely fixes
1. **DXF import (`art.import_dxf`):** the `dxf` crate needs uuid → getrandom, so `commands/Cargo.toml` makes it native-only. uuid 1.29 already pulls js-sys and wasm-bindgen on wasm, so enabling uuid's `js` feature or getrandom 0.4's `wasm_js` probably fixes it. Untested.
2. **PDF and PDF-based AI import:** lopdf is native-only because its encryption support needs randomness (getrandom 0.4). The same getrandom fix may work; otherwise those imports stay off.
3. **Fonts:** the `system` feature (fontdb, font folders) is off, so only Hershey fonts exist. `text::registry::add_font_data` exists, but no command or route calls it. It needs a wasm export or new command, with uploaded fonts kept in IndexedDB. The Local Font Access API is an option on Chromium only. Imported LightBurn text that uses fonts like Arial will show as missing fonts.
4. **Clock:** `commands::builtin::text::now()` returns None on wasm, so date/time variable text shows its format instead of a date. Fix with chrono's `wasmbind` or a moment passed in from JS.
5. **Threads and blocking work:** the pure crates use no threads (no rayon), but 50–100 MB imports, trace, nest and plan are synchronous. The engine runs them on `spawn_blocking`; the browser should run the wasm in a Web Worker.
6. **PNG rendering:** `render::svg_to_png` uses resvg, which is an engine-only dependency and was not checked for wasm. Canvas rendering is the alternative.
7. **Filesystem:** projects, autosave, backups, templates, art and material libraries, presets, prefs and devices all live on disk. They move to IndexedDB, or to file download/upload.
8. **Memory:** each timeline step keeps a full Document snapshot (up to 500). Image data is `Arc<str>`, so snapshots are cheap inside wasm. The cost is serializing the document across the worker boundary on every refetch.

## Engine-only pieces an in-browser backend must re-create
- **Active Session and multi-document set:** one `commands::Session` per document (wasm-ready), the DocSet with ids, and the revision raise on activation.
- **ProjectState:** name, display_name, saved and clean revision, and `dirty`, as reported by `GET /api/project`.
- **`batch_opening` wrapper:**
  - the risk gate;
  - the prefs layer and optimization defaults;
  - the `doc.new` and `doc.import_lbrn2` field/workspace fill from the active device or the LightBurn DeviceName;
  - `field_source`;
  - the effect of `replaces()` on project state;
  - and the announce events.

  The pure helpers in `state.rs` can move to a shared crate: `new_document_for`, `galvo_field`, `new_layout`, `lightburn_layout`, `fill_layout`, `fill_field_mm`, `sniff_image`, `replaces`.
- **Upload and import routing:** `UploadFormat` / `import_invocation`, `import_layer` (tool layers), the replace = `doc.new` + import batch, `projects::display_name`, and the `saveable` round-trip check.
- **Preview:** coalescing, Target from the active device (`hw::target_from_profile`, which can move since machine compiles), `run_index`, the marktime factor, the unsupported-settings annotation, and SVG/PNG rendering (`render.rs` is pure apart from resvg).
- **Device registry:** at least a design-only device, and stub `/api/machine`, `/api/job` and `/api/devices` answers.
- **Stores:** PrefStore and file settings, LibraryStore (.clb), ArtStore (.oeart/.lbart with thumbnails), material-test and image-adjust presets, templates, autosave timer and backups.
- **Events bus:** replaces the tokio broadcast channel and the WebSocket.
- **Dropped entirely:** security guard and token, PIN and approval, runs, job tracker and log, framing, hardware links, camera, updates, diagnostics, shutdown, MCP.

## What the UI rework may change (read-only diff against main)
- **ui-p4:**
  - `web/src/api/client.ts` gains token state and a Connect card (a 401 sets `needsToken`). The design-only build must never show it.
  - `security.rs` and `main.rs` change.
  - `vite.config.ts` changes (manifest, app icons, precompressed build), which affects the GitHub Pages `base` and asset paths.
  - No service worker in any rework branch, so a service-worker shim would not clash.
- **ui-p1:** rewrites `App.tsx` (+726/−373: docks, layout modes, context strip, bottom sheet). That is where the machine and laser docks will need hiding in design mode.
- **ui-base, ui-p2, ui-p3:** only a 7-line `App.tsx` change each.
- **Engine routes:** no rework branch adds or renames any.

The LightBurn terms-of-service question was outside this step and was not assessed.

Files that matter most:
- `crates/engine/src/api.rs`
- `crates/engine/src/state.rs`
- `crates/engine/src/userfiles.rs`
- `crates/engine/src/documents.rs`
- `crates/engine/src/artlib.rs`
- `crates/engine/src/render.rs`
- `crates/engine/src/events.rs`
- `crates/commands/Cargo.toml`
- `crates/fileio/Cargo.toml`
- `crates/text/src/registry.rs`
- `web/src/api/client.ts`
- `web/src/api/useEngine.ts`