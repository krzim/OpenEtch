# Architecture

OpenEtch is a laser design and control app for BJJCZ LMC (EZCAD2-family) galvo fiber lasers, built first for a
ComMarker B6 MOPA 60W. It has a Fusion 360-style UI, a Rust engine, and an AI-first command API (MCP).

Background research: `../research/oss-survey.md`.

## Deployment

One engine binary (`crates/engine`) runs either on a Raspberry Pi 4/5 that sits on the laser and is plugged into it by
USB, or on a Mac/PC cabled to the laser. It serves the web UI and an MCP endpoint over the network. Browsers (iPad,
Mac, PC, phone) and Claude are clients of the same live document. The engine is the only process that touches USB.

## Crates

| Crate | Role | Depends on | Native-only? |
|---|---|---|---|
| `lmc` | LMC wire protocol: opcodes, 12-byte single commands, 3072-byte list packets, unit conversions, list builders, decoder/disassembler. **Pure, no I/O.** | – | no (wasm-ok) |
| `corfile` | EzCad lens correction files (`JCZ_COR_2_1`, `LMC1COR_1.0`): parse, produce the WriteCorLine upload sequence, host-side bilinear correction. **Pure.** | – | no |
| `doc` | Document model: stock, layers with MOPA cut settings, objects (kurbo paths + affine transforms). **Pure data + geometry helpers.** | kurbo | no |
| `lbrn` | LightBurn `.lbrn2` projects and `.clb` material libraries: parse, write, convert to and from `doc` (see "File formats"). **Pure.** | doc, kurbo, roxmltree | no |
| `commands` | Command registry: every change to a document goes through a registered command with a JSON Schema. Session with undo/redo and an origin-tagged history. | doc | no |
| `plan` | Document → job: flatten paths, hatch fills, order travel, encode to LMC list packets for a `MachineProfile`. Also simulation (decode packets to a preview). | doc, lmc | no |
| `machine` | Machine profile (TOML, importable from EzCad `markcfg7`), the `Link` transport trait, `SimLink`, `UsbLink` (nusb, feature `usb`), `Controller` (init, .cor upload, light framing, run, abort). | lmc, corfile | `usb` feature only |
| `engine` | Binary. axum HTTP + WebSocket API for the web UI, MCP server (rmcp), run approval, serves `web/dist`. | all | yes |
| `web/` | Fusion-style UI: React + TypeScript + Vite. Talks to the engine over HTTP + WebSocket. | – | – |

## Coordinate frames and units

- All public APIs use **millimetres** and **degrees**.
- **Field frame**: origin at the centre of the galvo field, +X to the operator's right, +Y away from the operator
  (toward the column), matching LightBurn's galvo frame. Objects, stock and jobs are all expressed in the field frame.
- Galvo units are only used inside `lmc`/`plan`/`machine`: `0x0000..=0xFFFF` across `field_mm`, centre `0x8000`.
- Axis swap / negate from the machine profile and lens correction are applied in `plan`/`machine` when converting
  field mm to galvo units, never in `doc`. Both use the one function pair `lmc::units::field_to_galvo` /
  `galvo_to_field` with a `FieldTransform { field_mm, swap_xy, negate_x, negate_y }`: negate in the field frame first,
  then swap; points with `|coord| >= field_mm / 2` are refused, never clamped. So the red-pointer frame and the burn
  always land in the same place.

## Safety model

**What the software can and cannot promise.** The hardware E-stop and key switch are the real safety system. The
checks below stop mistakes and keep an AI agent that only has the MCP tools from firing the laser. They do **not**
stop a local process running with the user's OS privileges: such a process (including an agent with a shell) can read
or replace the PIN hash file (and guess the PIN offline from it), drive the browser, talk to the engine like the UI
does, restart the engine with other options, or open the USB device itself. Only the hardware E-stop/key switch holds
against that. The optional physical confirmation input is enforced by the engine: it protects against a guessed or
stolen PIN used through the engine's API, not against a process that restarts the engine or opens USB itself.

- Risk classes on every command: `Query`, `Edit`, `Pointer` (red light / motion, no emission), `Emit` (laser fires).
  `/api/execute`, `/api/batch` and MCP `execute`/`batch` accept only `Query` and `Edit`.
- Light framing uses a code path that **cannot** open the master oscillator or set the laser-enable port bit: it only
  emits jumps and the red-pointer port bit.
- An `Emit` action never runs from an API or MCP call directly. `POST /api/runs` / MCP `job_request_run` plan, encode
  and check the document into a *pending* run and send nothing. Only `POST /api/runs/{id}/approve` with the
  **operator PIN** turns it into an armed run (`approve` is the only engine module that creates `machine::Armed`, the
  token `Controller::run` needs). The endpoint refuses requests carrying MCP client headers or a non-`ui` origin, and
  a run whose document revision is stale.
- **Operator PIN.** Set by a person with `openetch set-pin` (interactive; refused unless stdin is a terminal;
  6-12 digits; shown while typed, because std has no portable no-echo input). Changing an existing PIN asks for the
  current one first; `openetch set-pin --reset` deletes the PIN only after the person types a confirmation phrase,
  and approvals then stay disabled until a new PIN is set and the engine restarts. Stored as a salted SHA-256 hash in
  `<config-dir>/operator-pin` (`--config-dir`, default the OS config dir + `openetch`, e.g. `%APPDATA%\openetch`,
  `~/.config/openetch`; on Unix the directory is created `0700` and the file `0600`, written through a `create_new`
  temp file, and a failed chmod is an error). The engine loads the hash **once** at startup. Every approval attempt
  (and `GET /api/machine`) checks the file's size, mtime and content hash; once it has changed (rewritten, deleted or
  created), approvals are refused with `pin_changed` until the engine restarts, and `approval.pin_changed` is true.
  Every 5 wrong PINs in a row lock approvals, each lockout twice the previous (60 s, 120 s, 240 s, ... capped at 1 h;
  only the right PIN or an engine restart resets the escalation, which is in memory); the engine emits `approval_locked` and reports
  `approval.locked_until_unix_s`. MCP never receives or handles PINs, and the MCP instructions tell agents they
  cannot and must not approve runs or set PINs.
- **Physical confirmation (optional).** With `--confirm-input <bit 0-15>` an approved run waits (state
  `awaiting_confirm`) up to 30 s, polling `ReadPort` (reply word 1, as galvoplotter reads it), until that input is
  seen released and then pressed for 3 reads in a row; a stuck, held or inverted input therefore never confirms. Stop
  cancels the wait; a timeout cancels the run. The B6's `markcfg` has `STARTMARKPORT=14`, which suggests a foot pedal
  on bit 14 (unverified, as is the active-high polarity).
- **Stop is never gated and never waits behind a run.** `POST /api/machine/stop` needs no token. It bumps a stop epoch
  (`AtomicU64`) and requests an abort before it waits for the controller; a run captures the epoch when it claims the
  machine and never starts streaming once it has changed, even if the stop's abort sequence reached the controller
  first. A running job notices the abort request at its next packet or status poll. If a run task panics, the engine
  sends the abort sequence (and `Controller::run` aborts from its own drop guard).
- Hard limits (max power, field bounds, mark speed) live in the machine profile and cannot be changed by command
  arguments. Layers may have at most 1000 passes (`commands` and `plan` both refuse more) and `plan_job` refuses jobs
  above 2,000,000 segments, so one edit cannot make planning exhaust the engine's memory. Previews are coalesced: at
  most one preview plan runs at a time, and requests queued behind it share one plan of the newest document.
- `machine::check_job` checks the encoded packets right before they are sent: power, frequency, pulse width, mark
  and jump speed, delays, no in-list `FiberOpenMO` (the MO is opened only by the run prelude), every `WritePort`
  either 0 or exactly the profile's laser-enable mask, and no raw opcodes.
- The engine holds no link of its own and reaches the board only through `Controller` methods: a freshly opened USB
  link is swapped in with `Controller::replace_link` (which forgets the init), and the physical confirmation reads
  inputs with `Controller::read_port`.

## Engine API (HTTP, JSON)

Errors are `{ "ok": false, "error": "..." }` with a real HTTP status (400, 403, 404, 409, 422, 502) unless the row
says otherwise. `origin` is `"ui"` (default), `"script"` or `"agent:<name>"`.

| Method | Path | Body / result |
|---|---|---|
| GET | `/api/commands` | `[CommandSpec]` (id, title, description, risk, input_schema) |
| POST | `/api/execute` | `{ "id": "...", "params": {...}, "origin": "ui" }` → `{ "ok": true, "result": ..., "revision": n }`; a failing command is **200** `{ "ok": false, "error", "candidates"? }` |
| POST | `/api/batch` | `{ "commands": [{id, params}], "origin": "ui" }` → `{ "ok": true, "results": [...], "revision": n }`; one undo step, all-or-nothing; failure as for execute |
| POST | `/api/undo` | optional `{ "origin" }` → `{ "ok": true, "revision": n }`; nothing to undo is **200** `{ "ok": false, "error" }`, so clients must check `ok` |
| POST | `/api/redo` | as `/api/undo` |
| GET | `/api/document` | full `Document` JSON |
| GET | `/api/history` | `[HistoryEntry]` (revision, command id, origin, summary) |
| GET | `/api/preview` | planned job: `{ segments: [{kind: "jump"|"mark", layer, from: [x,y], to: [x,y]}], stats }` in field mm; an empty document is an empty preview; a planning error (out of field, power above the limit, too many passes/segments) is **422** |
| GET | `/api/preview.svg` | rendered preview (stock, objects, toolpath; `?jumps=false` hides travel); still renders the document on a planning error |
| GET | `/api/machine` | `connected`, `simulated`, `busy`, `state`, `running`, `framing`, `field_mm`, `profile`, `max_power_pct`, `last_error`, `sim_counters`, `approval: { pin_set, pin_changed, locked_until_unix_s: null \| n, physical_confirm: null \| { input_bit, timeout_s } }`, `pending_runs: [Run]`, `runs: [Run]` |
| POST | `/api/runs` | optional `{ "origin" }` → **201** `{ "ok": true, "id", "run": Run, "message" }`; plans, encodes and checks the document; sends nothing to the board; at most 8 pending |
| POST | `/api/runs/{id}/approve` | `{ "pin": "<operator PIN>" }` → `{ "ok": true, "id", "state": "approved" \| "awaiting_confirm" }`; the run then starts in the background. PIN refusals are **403** `{ "ok": false, "error", "code": "pin_not_set" \| "pin_changed" \| "pin_required" \| "pin_invalid" \| "pin_locked" }`; MCP headers or a non-`ui` origin 403; unknown run 404; not pending, stale revision or another run active 409 |
| POST | `/api/machine/frame` | `{ "ids"?: [...], "mode"?: "bounds" \| "outline" }` → red-pointer framing (light only) until frame stop, stop, a run, or 120 s |
| POST | `/api/machine/frame/stop` | ends framing only; pending and approved runs are untouched → `{ "ok": true }`; with no board connected **200** `{ "ok": true, "connected": false }` |
| POST | `/api/machine/stop` | emergency stop: abort any run or frame, cancel every run not yet streaming (`pending`, `approved`, `awaiting_confirm`; `cancelled_runs` counts them) → `{ "ok": true, "connected": true, "cancelled_runs": n }`; with no board connected **200** `{ "ok": true, "connected": false, ... }`; no token needed |
| GET | `/api/projects` | `[{ name, modified_unix_s, size }]`: the saved projects, sorted by name; a missing projects directory lists nothing |
| POST | `/api/projects/save` | `{ "name", "overwrite"?: bool (default true) }` → `{ "ok": true, "name", "path", "revision" }`; writes `<projects-dir>/<name>.openetch` and makes it the current project; emits `project_saved`. With `overwrite: false` an existing project other than the current one is **409** |
| POST | `/api/projects/open` | `{ "name", "origin"? }` → `{ "ok": true, "name", "result", "revision" }`; replaces the document (`doc.load`, one undo step) and makes it the current project; unknown project 404, unusable file 400 |
| GET | `/api/project` | `{ "name": string \| null, "saved_revision": n \| null, "revision": n, "dirty": bool }`; refetch on `document_changed` and `project_saved` |
| GET | `/api/document/download` | `?format=openetch` (default) \| `lbrn2` → the document as a file with `Content-Disposition: attachment` (named after the project, else the document; ASCII `filename` plus UTF-8 `filename*`), `Cache-Control: no-store` and `X-OpenEtch-Warnings: <count>` (LightBurn conversion warnings; the texts come from `doc.export_lbrn2`). LightBurn files are laid out for the document's `field_mm` |
| POST | `/api/document/upload` | `?format=openetch\|lbrn2&mode=replace\|merge&field_mm=<mm>` with the raw file as the body (UTF-8, a BOM is dropped, at most 50 MB, else **413**) → runs `doc.load` (`.openetch`, replace only) or `doc.import_lbrn2` (`field_mm` only for LightBurn; default the document's) with origin `ui`, answered like `/api/execute` (a refused file is **200** `{ "ok": false, "error" }`). Unknown query parameters, formats or modes are 400 |
| WS | `/api/events` | server → client JSON events (below) |
| * | `/mcp` | MCP Streamable HTTP endpoint |

`Run` = `{ id, state, origin, revision, stale, summary, stats, packet_count, created_unix_s, error?, report? }`.
`state` is `"pending"` → `"approved"` → (`"awaiting_confirm"` →) `"running"` → `"finished"` \| `"failed"` \|
`"aborted"` (stopped while streaming), or `"cancelled"` (stopped before streaming, stale, or physical input not
pressed in time). `stale` is true for a pending run whose document has changed since; it cannot be approved.

Events on `/api/events`:

- `{ "type": "document_changed", "revision": n, "origin": Origin }` (after an edit, undo or redo; a client that fell
  behind gets `revision: -1, origin: null` and should refetch everything)
- `{ "type": "machine", "event": "framing" | "frame_stopped" | "run_approved" | "awaiting_confirm" |
  "run_started" | "run_finished" | "stopped" | "error" | "lagged", "detail"?: "..." }` (refetch `GET /api/machine`)
- `{ "type": "run_pending", "run_id", "origin", "summary" }`
- `{ "type": "run_finished", "id", "ok": bool, "state", "error": string | null }`
- `{ "type": "approval_locked", "until_unix_s": n }` (too many wrong PINs; approvals locked until then)
- `{ "type": "project_saved", "name", "revision" }` (refetch `GET /api/project`)

**Projects.** Named projects are `<projects-dir>/<name>.openetch` (`--projects-dir`, default the OS data dir +
`openetch/projects`, e.g. `%APPDATA%\openetch\projects`, `~/Library/Application Support/openetch/projects`,
`~/.local/share/openetch/projects`; created on the first save). A name is 1-100 characters of letters, digits, space,
`-`, `_`, `.`, `(` and `)`, with no leading dot, no `..`, no leading or trailing space, and not a Windows device name
(`con`, `nul`, `com1`, ...). Every file is also resolved against the canonical projects directory and refused unless
it stays inside it, so links pointing elsewhere are neither read nor written through. Saves go to a fresh temporary
file that is renamed over the old one. The engine tracks the current project (name, saved revision) in memory: save
and open set it, `doc.new` and `doc.load` (including an `.openetch` upload) clear it, and a LightBurn import keeps
it. The document is dirty whenever its revision differs from the last save, open or `doc.new` (undoing back to the
saved state still counts as a change).

**Field size.** The engine starts with `document.field_mm` set to the machine profile's `field_mm`. `doc.new` and
`doc.load` keep it when the new document has none and imports fill it in, so it stays set; when it is ever missing,
`doc.import_lbrn2` and `doc.export_lbrn2` called without `field_mm` get the profile's.

**Access.** Every request's `Host` must be a loopback name, an IP literal or an `--allow-host` name, and an `Origin`
header, when present, must be loopback, allowed or same-host (DNS-rebinding and cross-site protection). On loopback
no token is needed. Off loopback (`--listen 0.0.0.0:7878` on the Pi) a bearer token is generated (or given with
`--token`) and printed in the startup URL: `/api/*` and `/mcp` need `Authorization: Bearer <token>`; only
`GET /api/events` (the WebSocket, where browsers cannot set headers) also accepts `?token=`. Static files and
`POST /api/machine/stop` need no token.

Default listen address `127.0.0.1:7878`. Other options: `--profile`, `--web-dir`, `--usb`, `--token`,
`--allow-host`, `--config-dir`, `--confirm-input`, `--projects-dir`; `openetch set-pin [--reset] [--config-dir <path>]` sets, changes
or deletes the operator PIN (restart the engine afterwards).

## MCP tools

`list_commands`, `execute`, `batch`, `inspect_document`, `render_preview` (PNG), `undo`, `redo`, plus separate machine
tools: `machine_status`, `frame_start`, `frame_stop`, `job_request_run`, `machine_stop`, and project tools:
`project_list` (`{projects, current}`), `project_save {name, overwrite?}` (replacing an existing project other than
the current one needs `overwrite: true`, default false), `project_open {name}` and `project_export_lbrn2 {name}`
(writes `<projects-dir>/<name>.lbrn2`, returns `{path, warnings, field_mm}`). Project tools take names, never paths.
File contents go through `execute` with `doc.save`, `doc.load`, `doc.import_lbrn2` and `doc.export_lbrn2`. The MCP
surface is generated from the same command registry the UI uses; machine tools are separate so clients can approve
them separately. No
MCP tool can approve a run or touch the operator PIN, and the server instructions say agents cannot and must not.

## File formats

- **`.openetch`** (native project): UTF-8 JSON `{"format": "openetch", "version": 1, "document": <doc::Document>}`
  (`doc::file`). Readers refuse other formats (XML is pointed to the LightBurn import) and newer versions, accept a
  UTF-8 BOM, refuse duplicate ids and raise `next_id` past existing ids. `Document.field_mm` is the galvo field the
  document was laid out for; `Layer.extras` keeps settings from other software that OpenEtch does not model (e.g.
  `lbrn2.*`) so exports round-trip. Commands: `doc.new`, `doc.load {content}`, `doc.save` (Query, returns
  `{content, revision}`).
- **`.lbrn2`** (LightBurn project, `crates/lbrn`): XML; mm, y up, origin at the lower-left of LightBurn's workspace,
  so field frame = LightBurn - `field_mm / 2`. The file does not record the field size it was laid out for, so
  import and export take `field_mm` (default the document's); a wrong value shifts everything by half the
  difference. CutSettings become layers (power, speed, frequency Hz to kHz, QPulseWidth ns, hatch, passes,
  sub-layers; everything else in `extras["lbrn2.*"]`), Rect/Ellipse/Path become objects, groups are flattened, text
  is imported as its outline (BackupPath), bitmaps and other shapes are skipped with a warning. Export keeps each
  layer's LightBurn index and writes the stock outline on tool layer T1. Commands: `doc.import_lbrn2 {content,
  mode?: replace|merge, field_mm?}` (Edit) and `doc.export_lbrn2 {field_mm?}` (Query, returns `{content, warnings,
  field_mm}`). The format notes are in `crates/lbrn/src/lib.rs`.
- **`.clb`** (LightBurn material library): read by `lbrn::parse_library` into layer settings.

## Licensing and provenance

Code ported from MIT/Apache projects keeps its notice in the file header and is listed in `ATTRIBUTION.md`. Never copy
from GPL/SSPL/unlicensed sources (Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader).
