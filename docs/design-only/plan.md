# OpenEtch Design (browser-only) build: plan

Plan against `main` @ `d9129a1`. Nothing in the repo was edited.

I checked the API map against the code, and it holds:
- **Native-only parts:** `commands/Cargo.toml` makes `dxf` and `text/system` native-only. `fileio/Cargo.toml` makes `lopdf` native-only. `builtin.rs:115,128` skips registering `ImportDxf`, `ImportPdf` and `ImportAi` on wasm32.
- **Fonts:** `text::registry::add_font_data(family, Vec<u8>)` exists at `registry.rs:85`, but nothing calls it.
- **Engine glue:** the pure helpers are all in `crates/engine/src/state.rs`: `replaces` :140, `sniff_image` :249, `batch_opening` with the risk gate :663/716, `new_document_for` :1377, `galvo_field`, `new_layout`, `lightburn_layout`, `fill_layout`, `fill_field_mm` :1429–1482. `render::preview_svg` is pure (render.rs:55); `svg_to_png` uses resvg (:171).
- **Events:** `events.rs` defines `Event` as `#[serde(tag="type", rename_all="snake_case")]`.
- **Web client:** `client.ts` has one `send()` → `fetch(path)` (:71) and `eventsUrl()` (:271). `useEngine.ts:252` opens the WebSocket and :343 polls the machine.
- **Lockfile versions:** wasm-bindgen 0.2.130, js-sys 0.3.107, uuid 1.29.0, getrandom 0.2.17, 0.3.4 and 0.4.3, resvg 0.49 with tiny-skia 0.12.
- **Other facts:** the machine crate depends only on lmc, corfile, serde and toml (not ruida). The bundled Hershey fonts total 76 KB. `web/package-lock.json` exists. There is no `.github/workflows` and no `rust-toolchain.toml` (MSRV 1.95). `vite.config.ts` has `sourcemap: true`.

---

## 0. Decisions in one place

| Question | Decision |
|---|---|
| Where the document engine runs | A **Dedicated Web Worker** that hosts `crates/webcore` (wasm). Not a service worker: the browser can stop a service worker at any time and lose the session. |
| How the UI talks to it | One **transport function**. The six fetch wrappers call `transport(method, path, init)` instead of `fetch`. In design mode it sends `{method, path, query, body bytes}` to the worker and gets back `{status, headers, body}`, which it wraps as a real `Response`. All `/api/...` paths, JSON shapes, `ApiError` handling, `Content-Disposition` downloads and normalisers stay as they are. |
| Same JSON as the engine | Move the engine's glue into a **shared crate `crates/appcore`** (no tokio, axum or std::fs) that both the engine and webcore call. A **parity test** sends the same scripted requests to the engine's axum router and to webcore's router and compares the JSON. |
| Events | The worker posts `{kind:"event", data}` after every mutation. A `FakeSocket` (onopen, onmessage, onclose, close) replaces `new WebSocket(eventsUrl())`, so the `useEngine` switch and `publishEngineEvent` stay unchanged. |
| Mode switch | Build time: `VITE_DESIGN_ONLY=1`, read once in `web/src/mode.ts` as `DESIGN_ONLY`. Machine UI sits behind `if (!DESIGN_ONLY)` and `lazy()` so Rollup drops it from the design bundle. One codebase, two `vite build` runs. Runtime detection (a Pages site that finds a local engine) is a later idea: Private Network Access and mixed-content rules make it a project of its own. |
| Storage | IndexedDB for projects, autosave, prefs, devices, libraries, presets, templates and fonts. File System Access (`showSaveFilePicker`) for Save and Save As to a real file where it exists (Chromium desktop). Elsewhere, `<a download>` and `<input type=file>`. |
| PNG rendering | Try **resvg in wasm** first. usvg is already linked for SVG import, so it adds only tiny-skia and gives the same pixels as the engine inside the worker. If that costs more than about 300 KB gz, rasterise on the main thread (`Image` + canvas; `createImageBitmap` cannot decode SVG in a worker). |
| Preview and time estimate | Plan against the active **design device** (a built-in profile, B6 MOPA by default): `run_index` 0, marktime factor 1.0, no lens calibration. The UI labels the time as "Estimate for <device>". |
| wasm toolchain | `cargo build --profile wasm-release --target wasm32-unknown-unknown`, then **wasm-bindgen-cli =0.2.130** (must match Cargo.lock exactly), then **binaryen `wasm-opt -Oz`** (pinned release). I'm not choosing wasm-pack: the rustwasm org was wound down and wasm-pack now lives elsewhere, and it is one more tool to pin. |
| Licences | wasm-bindgen, js-sys, web-sys, wasm-bindgen-futures, console_error_panic_hook and getrandom are all MIT OR Apache-2.0. binaryen is Apache-2.0 and only runs at build time. No new npm runtime dependencies. Add `cargo-deny` (MIT/Apache) with a licence allowlist in CI. Any bundled substitute fonts (Liberation) are OFL-1.1: the owner must agree to that, and `OFL.txt` ships with them. |

---

## 1. In-browser backend: `crates/webcore` (+ `crates/appcore`)

### 1.1 `crates/appcore` (shared glue, wasm-clean)
This code moves out of `crates/engine/src`. The engine keeps its axum handlers as thin wrappers.

- **Session set:** `DocSet` (from documents.rs): ids, the active document, `raise_revision` high-water mark on activate, close and open.
- **`ProjectState`:** name, display_name, saved/clean revision and dirty, as `GET /api/project` reports them.
- **`open_batch(...)`** (the core of `batch_opening`):
  - the Query/Edit risk gate;
  - layer and optimization defaults from a `Prefs` value passed in (`libs::fill_defaults` logic);
  - `doc.new` / `doc.import_lbrn2` field and layout fill: `new_document_for`, `galvo_field`, `new_layout`, `lightburn_layout`, `fill_layout`, `fill_field_mm`, `lbrn2_device_name`;
  - the LightBurn `field_for_file_device` lookup over a device list passed in;
  - `replaces()` and its effect on ProjectState;
  - the `field_source` annotation;
  - it returns `Vec<Event>` and never broadcasts itself.
- **Upload and import routing:** `UploadFormat`, `import_invocation`, `import_layer` (tool layers), replace = `doc.new` + import in one batch, `sniff_image`, `projects::display_name`, and the `saveable` round-trip check.
- **Preview assembly:** `target_from_profile` (from engine `hw.rs`), `plan_job_with` / `preview_document` / `scale_times` / `retain_layers`, and the `unexecuted_settings` annotation. The marktime factor is a parameter.
- **`render::preview_svg`:** the pure part of render.rs. `svg_to_png` stays behind a `png` feature that pulls in resvg.
- **Typed stores:** in-memory PrefStore, FileSettings, LibraryStore (.clb), ArtStore (.oeart/.lbart with thumbnails), material-test and image-adjust presets, templates index, and the device list. Each mutation returns the store writes it needs. The engine applies them with std::fs and webcore hands them to JS.
- **`Event` enum:** moves here from events.rs. The engine keeps the broadcast channel.

The cheaper option is to copy this glue into webcore. That saves about 2–3 days but the two copies will drift. It is only worth it if engine changes must wait for the rework.

### 1.2 `crates/webcore` (cdylib + rlib)
- **Dependencies:** appcore, commands, plan, machine (default features: no usb or serial), fileio, lbrn, text, `wasm-bindgen = "=0.2.130"`, js-sys, console_error_panic_hook. Not serde-wasm-bindgen: everything crosses as JSON strings or `Vec<u8>`, which is smaller and simpler.
- **Exports:**
  ```text
  Backend::new(init_json: &str) -> Backend        // prefs, devices, libraries, presets, fonts index, design device
  Backend::handle(method, path, query, body: &[u8]) -> Reply {status, content_type, disposition?, body: Vec<u8>}
  Backend::drain_events() -> String               // JSON array of Event
  Backend::drain_writes() -> String               // JSON array of {store, key, op, value|bytes_ref}
  Backend::add_font(family: &str, bytes: Vec<u8>) -> String   // text::registry::add_font_data
  Backend::set_clock(unix_ms: f64, tz_offset_min: i32)
  Backend::png(svg: &str, width: u32) -> Vec<u8>  // if resvg is in
  ```
- **The router** (a hand-written match, no axum) covers this subset:
  - commands, execute, batch, undo, redo, history and its goto/delete;
  - document, documents and their activate/close;
  - layers/unsupported;
  - `preview` and `preview.png`;
  - project, plus download/upload/import/export/print (export includes `export/gcode` only with a GRBL design profile, which is optional);
  - images/{id}/file and processed.png;
  - projects/thumbnail (the PNG from an `.openetch` passed in the body);
  - art libraries, material libraries, prefs (+ layer/optimization defaults, export/import, import_lightburn), templates, presets;
  - `/api/lightburn` (upload path only), and `/api/devices` (list, get, activate, and import of `.lbdev` / prefs.ini device lists from upload);
  - `.oebundle` export/import with the parts prefs and libraries only;
  - **stubs:** `GET /api/machine` → `{connected:false, design_only:true, runs:[], pending_runs:[]}`, `GET /api/job` → idle, everything else machine-related → 404 `{error:"not available in the design-only build", code:"design_only"}`.
- **Projects, backups and templates bodies** (large, so loaded on demand) are routed in the **TS worker shell**, which reads them from IndexedDB and calls `handle("POST","/api/document/upload?format=openetch",bytes)`. Save works the other way: `GET /api/document/download?format=openetch`, then the shell writes the bytes to IDB together with a thumbnail made by `png()`. This keeps Rust synchronous and testable natively (`cargo test -p webcore`).
- **Panics:** `panic = "abort"` poisons the instance after a panic. The shell then restarts the worker, re-opens the last autosave and shows "The editor restarted; recovered autosave from hh:mm".

### 1.3 Fixing the wasm blockers
1. **DXF:** enable getrandom's wasm backend on wasm32 only (`getrandom 0.2` feature `js`, and the matching feature or `--cfg getrandom_backend="wasm_js"` for 0.3/0.4, whichever versions `dxf`/`uuid` pull), then drop the `cfg(not(wasm32))` gates on `dxf` and `ImportDxf`. Untested: WP0 decides.
2. **PDF and AI:** the same fix for lopdf. If it fails, PDF/AI import stays off in the design build and the import dialog says so (the `commands.ts` startup check must accept that).
3. **Clock:** add a `text` feature `wasm-clock` = `chrono/wasmbind` + `chrono/clock`, or keep `now()` and give it the moment from `set_clock`. The feature is simpler; the parameter is easier to test.
4. **Fonts:** fonts come from four places:
   - The bundled Hershey fonts (always).
   - **User fonts:** TTF/OTF from the Font picker, stored in IndexedDB `fonts`, then `add_font` at boot. `font.list` must include them; check `registry` lists added data fonts.
   - **Chromium:** the Local Font Access API (`queryLocalFonts`) on an explicit button.
   - **Optional:** lazy-loaded Liberation Sans, Serif and Mono (Latin subset, about 100–150 KB each) as metric-compatible stand-ins for Arial, Times and Courier in imported LightBurn files. This needs the owner's OK for OFL.

   The missing-font warning names the font and offers "Upload font…".
5. **Threads:** the whole backend runs in one worker, so UI input stays responsive during 50–100 MB imports, trace, nest and plan. No SharedArrayBuffer is needed; Pages cannot send COOP/COEP headers anyway.

### 1.4 Size budget and how to keep it small
These numbers are estimates; WP0 measures them.

| Piece | Target (over the wire, gzip) |
|---|---|
| `webcore_bg.wasm` after `wasm-opt -Oz` | **≤ 2.5 MB** (hard fail in CI at 3.5 MB) |
| wasm-bindgen JS glue + worker shell | ≤ 30 KB |
| UI JS (design build, machine UI dropped) | ≤ 350 KB, below today's full bundle |
| Optional substitute fonts | Loaded on first use only |

Ways to keep it small:
- A `[profile.wasm-release]` that inherits release with `opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"` and `strip=true`, used only for wasm so the engine's speed is unchanged.
- `wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int`.
- A `twiggy top` report in CI (artifact only).
- Strings and bytes at the boundary instead of JsValue reflection; no `web-sys` unless needed.
- `machine` with default features only (lmc and corfile are small, no ruida).
- resvg only if WP0's measurement allows it.
- Later, if needed: split rarely used importers (3D model/Slice3d, EZD, Gerber) into a second lazily loaded wasm. Not in v1.
- Start the wasm download with `WebAssembly.instantiateStreaming` (Pages serves `application/wasm`) in parallel with the UI bundle, and import it via Vite `?url` so the file name carries a content hash.

---

## 2. Web app changes (one codebase)

New files:
- **`web/src/mode.ts`:** `export const DESIGN_ONLY = import.meta.env.VITE_DESIGN_ONLY === "1";` plus `FULL_APP_URL` and `REPO_URL` from env.
- **`web/src/api/transport.ts`:**
  - `transport(method, path, init): Promise<Response>`: `fetch` when native, worker RPC when design-only. Request ids are matched to replies; bodies are transferred as `ArrayBuffer`.
  - `openEventStream(): SocketLike`: a WebSocket when native, a FakeSocket fed by worker `event` messages when design-only.
- **`web/src/design/backend/`:** a new folder (the name clashes with the existing `web/src/design/`, so it could instead be `web/src/standalone/`). Files:
  - `worker.ts`: boots the wasm, routes storage requests, runs the autosave timer;
  - `rpc.ts`;
  - `idb.ts` behind a `KeyValueStore` interface with an in-memory version for tests;
  - `fsaccess.ts`: File System Access handles kept in IDB, with permission asked again on Save;
  - `fonts.ts`.

Edits:
- **The six wrappers** (`client.ts` `send`, `jobApi.ts`, `design/designApi.ts`, `files/filesApi.ts`, `libs/api.ts`, `workspace/wsApi.ts`): replace `fetch(` with `transport(` (one line each).
- **`useEngine.ts`:**
  - `new WebSocket(eventsUrl())` becomes `openEventStream()`;
  - skip the `/api/machine` polling when `DESIGN_ONLY` (the stub stays as a safety net);
  - `JobControls`' `/api/job` polling disappears with the hidden UI.
- **`api/commands.ts` startup check:** in design mode, a missing `importDxf` or `importPdf` disables the matching import entries instead of reporting an error.

**Hide when `DESIGN_ONLY`:**
- LaserDock / LaserPanel, JobControls / jobStore runs, MachineJog, GrblConsole, GrblMove;
- FramingOptions, positionLaser, LensWizard, RotaryDialog;
- FindLaser, Camera, JobSafety, LaserExtras, the run history;
- StatusBar machine state and the TopBar connection state;
- Devices scan, wizard and `.cor`/markcfg import;
- reading LightBurn prefs on the engine's own computer (keep the upload);
- HelpMenu entries for MCP (HelpMenu.tsx:158), diagnostics and updates;
- the **Connect/token card from ui-p4**: the shim never returns 401, but gate it explicitly too.

**Show instead:**
- **A header strip:** "OpenEtch Design: runs in your browser, nothing is uploaded" · "Get the full app (laser control)" → GitHub repo or releases.
- **Export buttons:** Save .openetch, **Export LightBurn .lbrn2**, SVG / DXF / PDF / AI, Print.
- **A Design device picker:** built-in profiles (B6 MOPA, generic 110/150/200/300 mm galvo fields), "Import from LightBurn .lbdev / prefs.ini" (in-browser parse of typed device fields only, so the privacy rules still hold), and field and workspace size in the DeviceEditor. Everything else on the device is read-only.
- **The Projects dialog** headed "Saved in this browser". Storage notices:
  - Safari may delete site data after 7 days without a visit unless the site is installed;
  - all `*.github.io` project sites of one account share storage;
  - "Download a copy" is offered on every save.

  Call `navigator.storage.persist()` on the first save.
- **Two tabs:** each tab gets its own worker. Saves use the existing 409 overwrite check against an IDB `modified` stamp, and a BroadcastChannel refreshes the other tab's project list and libraries.

**Vite:**
- `base: process.env.VITE_BASE ?? "/"`;
- `sourcemap: !DESIGN_ONLY` (sourcemaps would publish the source of a private repo);
- the worker via `new Worker(new URL("./worker.ts", import.meta.url), {type:"module"})`;
- no dev proxy in design mode;
- the ui-p4 manifest uses relative `start_url`/`scope`, and its precompression is skipped for Pages (Pages compresses with gzip itself and ignores `.br` files).

---

## 3. Build and deploy

### 3.1 Workflow `.github/workflows/design-site.yml`
All actions are pinned by commit SHA.

- **Triggers:** `push` to `main` with paths `crates/**`, `web/**`, `Cargo.lock`, and the workflow file; plus `workflow_dispatch`.
- **Job `build`** on `ubuntu-24.04`:
  1. `actions/checkout`.
  2. `dtolnay/rust-toolchain` with toolchain `1.95.0` and target `wasm32-unknown-unknown`.
  3. `Swatinem/rust-cache` with `key: wasm`, so it is cached separately from any native jobs.
  4. Install `wasm-bindgen-cli@0.2.130` and `binaryen` (a pinned version) with `taiki-e/install-action` (prebuilt binaries, cached by the tool cache). A step **fails if `wasm-bindgen --version` ≠ the version in Cargo.lock**.
  5. `cargo build -p webcore --profile wasm-release --target wasm32-unknown-unknown --locked`.
  6. `wasm-bindgen --target web --out-dir web/src/standalone/pkg …`, then `wasm-opt -Oz`.
  7. Size gate: `gzip -9 | wc -c` must be ≤ 3.5 MB; print `twiggy top -n 30`.
  8. `cargo deny check licenses` and `cargo test -p appcore -p webcore` (native, fast).
  9. `actions/setup-node` (Node 22, `cache: npm`, `cache-dependency-path: web/package-lock.json`), `npm ci`, `npm test`.
  10. `VITE_DESIGN_ONLY=1 VITE_BASE=/<site-path>/ npm run build`.
  11. Copy LICENSE, ATTRIBUTION.md and a generated `THIRD-PARTY.html` (`cargo about generate`; cargo-about is MIT/Apache) into `dist`.
  12. Add `dist/.nojekyll`.
  13. `actions/upload-pages-artifact`.
- **Job `deploy`**, only for options A/B below: `needs: build`, `permissions: {pages: write, id-token: write}`, environment `github-pages`, `actions/deploy-pages`.

### 3.2 Repository options

| Option | How | Cost and exposure |
|---|---|---|
| **A. Make OpenEtch public** | Pages source = GitHub Actions in this repo. URL `krzim.github.io/OpenEtch/`. | Free and simplest. The whole source is public: check ATTRIBUTION, fixtures (`jcz150.cor`, `markcfg7.ini` are the owner's machine files) and env-gated sample paths first. |
| **B. Private repo + GitHub Pro** | Pages from a private repo (paid plan). The **site is still public**; only Enterprise Cloud can restrict access. | About $4/month. Only the built JS and wasm are exposed, so sourcemaps must be off. |
| **C. Separate public repo `krzim/openetch-web`** | The private repo's workflow builds and **pushes `dist` to `openetch-web`'s `gh-pages` branch**, which Pages serves as "Deploy from a branch". Auth options: **(preferred)** a write **deploy key** on openetch-web whose private half is the secret `OPENETCH_WEB_DEPLOY_KEY` in OpenEtch (scoped to that one repo, no expiry, not tied to a person); or a fine-grained PAT with `contents: write` on openetch-web only (it expires). `actions/deploy-pages` can only publish to the repo it runs in, so option C uses a branch push. Alternatively, upload a release asset and let a tiny workflow in openetch-web run deploy-pages. | Free. The public repo holds only built files, a README ("built from a private source"), LICENSE and notices. Keep the history short (force-push a single commit, or `--depth 1`). |

My recommendation: C now, A when the owner decides to open the source; the workflow differs only in its last job. Every option ships LICENSE, ATTRIBUTION.md and THIRD-PARTY.html. MIT and Apache require the notices to travel with the binaries.

### 3.3 Caching
- **CI:** rust-cache keyed on Cargo.lock and the toolchain; cached prebuilt tools; npm cache. A cold build is roughly 6–10 minutes, a warm one 2–4.
- **Browser:** Pages sends `Cache-Control: max-age=600` for everything and its headers cannot be changed. Content-hashed file names (Vite assets and the wasm through `?url`) mean a new deploy is picked up within 10 minutes and nothing stale mixes. A PWA service worker for offline use is a later option; no rework branch adds one, so it would not clash.

---

## 4. Formats: LightBurn and reverse engineering

**`.lbrn2` export is in.** The writer in `crates/lbrn` is OpenEtch's own. Its sources are format facts from MIT projects (openlaser-rs, lbrnts, lbrn2-to-svg), LightBurn's public documentation and forum, and checks against the owner's files. It contains no LightBurn code or assets (ATTRIBUTION.md, "Never copied").

**ToS:** I did not read LightBurn's EULA or ToS, and this is not legal advice. Writing a file format for interoperability is a different thing from reverse engineering the program. The owner should read the current EULA's reverse-engineering clause and decide.

Branding guidance either way:
- use "LightBurn" only descriptively ("exports LightBurn .lbrn2 files");
- add "OpenEtch is not affiliated with LightBurn Software";
- no LightBurn logos or screenshots.

**Out of the public site:**
- **`.lbzip` bundles**, import and export. The manifest items `import-cutlib`, `import-imagepresets` and `import-hotkeys`, their file names and icons were **inferred from strings in LightBurn 2.2's executable** (`crates/engine/src/lbzip.rs:24–40`, ARCHITECTURE.md:1121, ROADMAP UX-068). That is program reverse engineering. Make sure webcore does not compile in `lbzip` (it stays in the engine) and the design UI has no `.lbzip` entries. A CI grep of `dist` for `LightBurnSoftware.json` and `import-cutlib` enforces this.

**Searched and not program reverse engineering.** These are formats worked out from files, or from third-party MIT notes; they are flagged for the owner's decision but OK to keep:

| Item | Basis | In design build? |
|---|---|---|
| `.lbart` art libraries (`fileio/lbart.rs`) | Decoded from two libraries saved by LightBurn with known contents (file layout only) | Keep |
| `.lbmt` material-test presets, `.lbimg` image presets, `.clb` material libraries | The owner's sample files, read in place, plus LightBurn's forum parameter list | Keep |
| `.lbprefs` / prefs.ini import (`lbprefs.rs`), `.lbdev` and prefs.ini device lists | The JSON file's keys; a whitelist of typed fields | Keep (in-browser parse) |
| Legacy `.lbrn` (`fileio/lbrn_legacy.rs`) | A LightBurn developer's forum description plus lbrnts | Keep |
| EzCad `.ezd` import (`fileio/ezd.rs`) | meerk40t `extra/ezd.py` (MIT), itself a third-party reverse engineering of **EzCad's file format**; checked against the owner's files | **Owner's call.** File format, not program. Keep unless the owner wants the public site free of anything EzCad-derived. |
| EzCad `.jsf` fonts (`text/jsf.rs`) | Layout worked out from the owner's EzCad fonts; no fonts shipped | Keep (users bring their own files) |
| ArUco MIP 36h12 | aruco_ros data, "checked against LightBurn's own markers 0–2" (comparing output, not reading code) | Keep |
| LMC single-command layout "inferred from two runs on the owner's B6" (`lmc/single.rs:261`), `.cor`/markcfg, Ruida | Machine protocol | Not reachable in the design build; Ruida is not linked. `lmc` and `corfile` are linked only because `machine` depends on them. |

---

## 5. Work packages

Effort is in builder-days. "Rework" means the UI branches ui-p1 (App.tsx docks) and ui-p4 (client.ts token, vite.config.ts).

| WP | Scope and owned files | Tests and acceptance | Effort | Depends on |
|---|---|---|---|---|
| **WP0 Spike and measure** | Throw-away `crates/webcore` skeleton: `doc.new` → `shape.rect` → `doc.export_lbrn2` through `handle`. Try the getrandom fixes for dxf and lopdf, chrono `wasmbind`, resvg on wasm32. | A report with the gz size of the wasm with and without resvg, and whether dxf, pdf and clock work. **Gate:** ≤ 3.5 MB gz, or a cut list. | 1.5–2 | – |
| **WP1 `crates/appcore` extraction** | New `crates/appcore/**`. Edits to engine `state.rs`, `documents.rs`, `userfiles.rs`, `projects.rs`, `render.rs`, `hw.rs` (target_from_profile), `events.rs`, `libs.rs`, `library.rs`, `artlib.rs`, `test_presets.rs`, `prefs.rs`, and the workspace `Cargo.toml`. | `cargo test -p engine` unchanged and green (wire_tests, userfiles_tests, app_tests); `cargo check -p appcore --target wasm32-unknown-unknown`. No route or JSON change: wire_tests prove it. | 4–6 | WP0. **Conflicts with any worktree that edits engine state.rs or userfiles.rs**: schedule it when those have merged. Independent of the UI rework. |
| **WP2 webcore router + exports** | `crates/webcore/**`: the router, stubs, the `Reply` type, event and write draining, the `png` feature. | Native `cargo test -p webcore`. **Parity test** `crates/webcore/tests/parity.rs`: about 60 scripted requests (new doc, shapes, text, image import + trace, LightBurn fixture import and export, SVG/DXF/PDF export, history goto/delete, preview, project save/open, library CRUD) against engine's `app()` via `tower::ServiceExt::oneshot` and against webcore; JSON equal after dropping volatile fields (timestamps, paths). | 5–7 | WP1 |
| **WP3 wasm blockers** | `crates/commands/Cargo.toml`, `crates/fileio/Cargo.toml` (getrandom backend on wasm32), `crates/text` (feature `wasm-clock`, data-font listing), the webcore `add_font` / `set_clock`. | The native test suite passes unchanged. Node runs the wasm (`wasm-bindgen --target experimental-nodejs-module` in a test script) to import `fixtures/*.dxf`, a PDF, and a TTF, then lays out text, shows `{date}` var text, and checks `font.list` includes the TTF. | 2–3 | WP0 |
| **WP4 worker shell and persistence** | `web/src/standalone/{worker,rpc,idb,fsaccess,fonts,autosave,routes}.ts` and tests. | `node --test`: storage routes against the in-memory KeyValueStore (save, list, open, 409 overwrite, autosave rotation of 10, backup restore, BroadcastChannel refresh). Manual: Chrome FS Access save, Firefox and Safari download fallback, reload keeps projects. | 4–5 | WP2 (its interface can be mocked earlier) |
| **WP5 transport + design-mode UI** | `web/src/mode.ts`, `web/src/api/transport.ts`, one-line edits in the six wrappers, `useEngine.ts`, `api/commands.ts`, design gating in App.tsx (docks, menus), TopBar, StatusBar, HelpMenu, Devices/DeviceEditor (design device picker), FileMenu/ExportDialogs (export emphasis, no `.lbzip`), ProjectDialogs, `vite.config.ts` (`base`, sourcemap, worker). | `npm test`, `npm run typecheck`, and the full build unchanged. Grep of the design `dist` must find none of `/api/machine/jog`, `/api/runs`, `/mcp`, `lbzip`, `LightBurnSoftware.json`. Manual smoke on the Pages preview at desktop and phone width. | 4–6 | **ui-p1 and ui-p4 merged first** (App.tsx rewrite, client.ts token card, vite.config changes). The transport and events part (about 1 day) can land earlier as a tiny PR that both branches rebase over. |
| **WP6 CI, deploy and notices** | `.github/workflows/design-site.yml`, `deny.toml`, `about.toml`/template, `Cargo.toml` `[profile.wasm-release]`, README section, the public repo README if option C. | The workflow builds and deploys on `workflow_dispatch` to the chosen target; the size gate and licence gate fail when they should (test with a deliberately bad PR). | 1.5–2 | WP2, WP5; the owner's choice of A/B/C and the deploy key secret |
| **WP7 Docs and QA** | ARCHITECTURE.md section "Design-only build", ATTRIBUTION.md (new deps and fonts), ROADMAP rows; a manual QA checklist. | Owner checks: open a real `.lbrn2` from LightBurn, edit, export `.lbrn2`, open it in LightBurn; a 50 MB image import and trace stays responsive. | 1–2 | all |

**Total: about 23–33 builder-days**, roughly 5–7 weeks for one builder. With two builders the WP2+WP3 track and the WP4+WP5 track run in parallel, about 3–4 weeks. Critical path: WP0 → WP1 → WP2 → WP5 (waits for the rework) → WP6.

### Risks
1. **wasm size** above budget, mostly from rxing, skrifa/harfrust, usvg, image codecs and i_overlay. Mitigations: WP0 gate, `-Oz`, a second lazy module, dropping resvg.
2. **Speed and memory:** wasm is single-threaded and maybe 1.5–3× slower than native for trace, nest and plan. wasm32 memory is capped at 4 GB, and 500 history snapshots with large images can approach it. Lower the step cap in the design build (for example 100) and show a progress indicator for long work.
3. **Engine and webcore JSON drift.** Mitigation: appcore plus the parity test in CI.
4. **Storage loss:** Safari evicts after 7 days, private windows lose everything, all `krzim.github.io` sites share storage. Mitigations: persist() request, "Download a copy" everywhere, the DB name prefixed `openetch-design`, a custom domain later if wanted.
5. **Collisions with the rework** in client.ts, App.tsx and vite.config.ts. Mitigations: land transport.ts first as a 6-line PR, do the UI gating after ui-p1/ui-p4.
6. **Collisions with other engine worktrees** during the WP1 extraction (large edits to state.rs). Coordinate and do it in one sitting.
7. **Exposure from a private repo:** sourcemaps (turned off), fixture machine files (never copied into `dist`), and the wasm's names section (stripped by `strip=true`).
8. **getrandom on wasm** may not fix dxf or lopdf (the three getrandom versions need different switches). Fallback: DXF and PDF import disabled in the design build, with a clear message.
9. **LightBurn trademark and ToS**, see §4. Owner decision.
10. **Panics** abort the wasm instance. Mitigation: worker restart plus autosave recovery.