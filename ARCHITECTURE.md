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
| `lbrn` | LightBurn `.lbrn2` projects and `.clb` material libraries: parse, write, convert to and from `doc` for a galvo field or a gantry bed (`lbrn::Frame`, see "File formats"). **Pure.** | doc, kurbo, roxmltree | no |
| `commands` | Command registry: every change to a document goes through a registered command with a JSON Schema. Session with a Fusion-style timeline (see "History timeline"). | doc | no |
| `plan` | Document → job: flatten paths, hatch fills, order travel, encode to LMC list packets for a `MachineProfile`. Also simulation (decode packets to a preview). | doc, lmc | no |
| `machine` | Machine profile (TOML, importable from EzCad `markcfg7` and from a LightBurn device, see "LightBurn devices"), devices (`machine::device`: LightBurn-style device model, family, workspace, origin), the `Link` transport trait, `SimLink`, `UsbLink` (nusb, feature `usb`), `Controller` (init, .cor upload, light framing, run, abort). | lmc, corfile | `usb` feature only |
| `engine` | Binary. axum HTTP + WebSocket API for the web UI, MCP server (rmcp), run approval, serves `web/dist`. | all | yes |
| `web/` | Fusion-style UI: React + TypeScript + Vite. Talks to the engine over HTTP + WebSocket. | – | – |

## Coordinate frames and units

- All public APIs use **millimetres** and **degrees**.
- **Field frame**: origin at the centre of the galvo field, +X to the operator's right, +Y away from the operator
  (toward the column), matching LightBurn's galvo frame. Objects, stock and jobs are all expressed in the field frame.
- **Gantry devices** (design-only, see "Devices") use the same frame: origin at the centre of the bed
  (`Document.workspace_mm` `[w, h]`), +Y away from the operator (toward the rear).
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
  arguments. Layers and sub-layers may have at most 1000 passes (`commands` and `plan` both refuse more), a layer at
  most 64 sub-layers (`commands`) and 100,000 planned passes in all (`plan`, counting cleanup runs), and `plan_job`
  refuses jobs above 2,000,000 segments, so one edit cannot make planning exhaust the engine's memory. Previews are coalesced: at
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
| POST | `/api/undo` | optional `{ "origin" }` → `{ "ok": true, "revision": n }`: moves the timeline marker back over one step (failed steps are skipped); nothing to undo is **200** `{ "ok": false, "error" }`, so clients must check `ok` |
| POST | `/api/redo` | as `/api/undo`, forward |
| GET | `/api/document` | full `Document` JSON |
| GET | `/api/history` | `{ "marker": n, "marker_moved_by": "edit"\|"undo"\|"timeline", "undone_steps": n, "steps": [Step] }` (see "History timeline"); string parameters over 512 bytes (file contents) are replaced by `"<n bytes omitted>"` |
| POST | `/api/history/goto` | `{ "marker": n, "origin"? }` → `{ "ok": true, "marker", "revision" }`: apply steps `1..=marker`, roll back the rest (0 = before the first step); a marker past the end is **200** `{ "ok": false, "error" }` |
| POST | `/api/history/delete` | `{ "index": n, "origin"? }` → `{ "ok": true, "marker", "revision" }`: remove step `index` (1-based) and replay the steps after it; no such step is **200** `{ "ok": false, "error" }` |
| GET | `/api/preview` | planned job: `{ segments: [{kind: "jump"|"mark", layer, pass, from: [x,y], to: [x,y], emits}], passes: [{layer_id, sub_layer, repeat, segments, mark_length_mm, jump_length_mm, estimated_time_s}], stats, layers: [{layer_id, name, color, mode, output, visible, passes, segments, mark_length_mm, jump_length_mm, estimated_time_s, sub_layers: [{sub_layer, name, label, cleanup, mode, power_pct, speed_mm_s, frequency_khz, pulse_width_ns, passes, planned_passes, segments, mark_length_mm, jump_length_mm, estimated_time_s}]}], warnings }` in field mm. `pass` indexes `passes`, the job's passes in burn order (a jump belongs to the pass it travels into); `sub_layer` is 0 for a layer's main settings and n for `sub_layers[n - 1]`; `layers` lists every document layer in order (zeros for layers with nothing to mark or output off; layer times leave out the job's start-up delay), each with its main settings and sub-layers (`label` "Main", the name, "Cleanup" or "Sub-layer n"; `passes` asked for, `planned_passes` planned, and their share of the layer's totals); `warnings` are things the plan left out (images). `?layers=id1,id2` keeps only those layers' segments and entries (stats and `passes` stay the whole job's). An empty document is an empty preview; a planning error (out of field, power above the limit, too many passes/segments) is **422** |
| GET | `/api/fill_regions` | `{ layers: [{layer_id, name, color, mode, visible, output, paths: [d], object_ids: [id]}] }`: for every layer whose mode is `fill` or `fill_then_line`, one SVG path per object (its closed subpaths, field mm, +Y up, flattened at 0.05 mm) to draw with `fill-rule: evenodd`, which is exactly the area the hatch covers. Display only |
| GET | `/api/preview.svg` | rendered preview (stock, objects, toolpath; `?jumps=false` hides travel); still renders the document on a planning error |
| GET | `/api/machine` | `connected`, `simulated`, `busy`, `state`, `running`, `framing`, `field_mm`, `profile`, `device` (the active device's summary, see `GET /api/devices`), `max_power_pct`, `last_error`, `sim_counters`, `approval: { pin_set, pin_changed, locked_until_unix_s: null \| n, physical_confirm: null \| { input_bit, timeout_s } }`, `lightburn: null \| { device, field_mm, diffs: [{field, base_value, lightburn_value, note}] }` (the active profile compared with the matched LightBurn device, see "LightBurn devices"), `pending_runs: [Run]`, `runs: [Run]` |
| GET | `/api/lightburn` | `{ available, source: "file" \| "uploaded" \| null, default_device: name \| null, devices: [{name, display_name, field_mm, width_mm, height_mm}] }` (only these fields); `default_device` is the device used when a LightBurn file names none |
| POST | `/api/lightburn/prefs` | LightBurn's `prefs.ini` as the raw body (at most 2 MB, else **413**): parses `DeviceList` in memory and stores only the parsed devices (`<config-dir>/lightburn-devices.json`), for an engine on a Pi without LightBurn; answers like `GET /api/lightburn`; no devices, not JSON or not UTF-8 is 400. Emits `machine` event `lightburn` |
| DELETE | `/api/lightburn/prefs` | forgets the uploaded devices (the local `prefs.ini` is used again, if any); answers like `GET /api/lightburn`; emits `machine` event `lightburn` |
| GET | `/api/devices` | `{ active: id, devices: [{id, name, display_name, source: "builtin" \| "lightburn-prefs" \| "lbdev" \| "manual", kind: "jcz_galvo" \| "gantry" \| "galvo_other" \| "unknown", lightburn_type, workspace_mm: [w, h], lightburn_field_mm: n \| null (galvos: the field LightBurn files are laid out for, LightBurn's Width, e.g. 153.8 on the B6 whose profile field is 153.75), origin: "center" \| "bottom_left" \| "top_left" \| "bottom_right" \| "top_right", controllable, reason: string \| null, active}] }` in list order (the built-in device first); `reason` says why a design-only device cannot be driven. See "Devices" |
| GET | `/api/devices/{id}` | the summary plus `settings` (LightBurn's Device Settings, read-only: `[{title, rows: [{label, key, value}]}]`, `key` the LightBurn key), `profile` (JCZ galvos: name, field, swap/negate, cor file, frequency range, limits, timing), `base` (the profile it was built on) and `diffs` (`[{field, base_value, lightburn_value, note}]`, the profile against that base); unknown id 404 |
| POST | `/api/devices/import` | `?add=all\|<name>` (default all) with a LightBurn `.lbdev` or `prefs.ini` as the raw body (at most 2 MB, else **413**), or `?from=lightburn` and no body: the devices of LightBurn's `prefs.ini` on the engine's computer (or the uploaded one) → `{ ok, added: [id], updated: [id], skipped: [{name, reason}] }`. Parsed with the typed, privacy-safe parser; a device with the same display name, controller, connection and size as an imported one is updated (same id). Not UTF-8, not JSON or no devices 400; `add=<name>` naming no device of the file 404. Emits `devices_changed` |
| DELETE | `/api/devices/{id}` | → `{ ok, id }`; the built-in or the active device 409; unknown 404. Emits `devices_changed` |
| POST | `/api/devices/{id}/activate` | optional `{ "origin" }` → `{ ok, active: summary, warnings: [text] }`: makes the device active (stored; it is also the one used at the next start). Refused **409** while a run is approved, awaiting confirmation or running, or while framing; MCP headers or a non-`ui` origin **403**. Cancels pending runs, swaps the machine profile (JCZ galvos; the board is initialised again, the simulator gets a fresh `SimLink`), and new documents start at the device's workspace. Emits `device_changed`. A correction file that cannot be found is a warning (output is then uncorrected) |
| POST | `/api/runs` | optional `{ "origin" }` → **201** `{ "ok": true, "id", "run": Run, "message" }`; plans, encodes and checks the document; sends nothing to the board; at most 8 pending |
| POST | `/api/runs/{id}/approve` | `{ "pin": "<operator PIN>" }` → `{ "ok": true, "id", "state": "approved" \| "awaiting_confirm" }`; the run then starts in the background. PIN refusals are **403** `{ "ok": false, "error", "code": "pin_not_set" \| "pin_changed" \| "pin_required" \| "pin_invalid" \| "pin_locked" }`; MCP headers or a non-`ui` origin 403; unknown run 404; not pending, stale revision or another run active 409 |
| POST | `/api/machine/frame` | `{ "ids"?: [...], "mode"?: "bounds" \| "outline" }` → red-pointer framing (light only) until frame stop, stop, a run, or 120 s |
| POST | `/api/machine/frame/stop` | ends framing only; pending and approved runs are untouched → `{ "ok": true }`; with no board connected **200** `{ "ok": true, "connected": false }` |
| POST | `/api/machine/stop` | emergency stop: abort any run or frame, cancel every run not yet streaming (`pending`, `approved`, `awaiting_confirm`; `cancelled_runs` counts them) → `{ "ok": true, "connected": true, "cancelled_runs": n }`; with no board connected **200** `{ "ok": true, "connected": false, ... }`; no token needed |
| GET | `/api/projects` | `[{ name, modified_unix_s, size }]`: the saved projects, sorted by name; a missing projects directory lists nothing |
| POST | `/api/projects/save` | `{ "name", "overwrite"?: bool (default true) }` → `{ "ok": true, "name", "path", "revision" }`; writes `<projects-dir>/<name>.openetch` and makes it the current project; emits `project_saved`. With `overwrite: false` an existing project other than the current one is **409**, also one whose name differs only in case |
| POST | `/api/projects/open` | `{ "name", "origin"? }` → `{ "ok": true, "name", "result", "revision" }`; replaces the document (`doc.load`, one undo step) and makes it the current project; a name differing only in case opens the one matching project (`name` in the answer is its name on disk; several matches are 409); unknown project 404, unusable file 400 |
| GET | `/api/project` | `{ "name": string \| null, "display_name": string \| null, "saved_revision": n \| null, "revision": n, "dirty": bool }`; `display_name` is the name of the file a replacing upload or LightBurn import came from, while no project holds the document; refetch on `document_changed` and `project_saved` |
| GET | `/api/document/download` | `?format=openetch` (default) \| `lbrn2` → the document as a file with `Content-Disposition: attachment` (named after the project, else the opened file (`display_name`), else the document; ASCII `filename` plus UTF-8 `filename*`), `Cache-Control: no-store` and `X-OpenEtch-Warnings: <count>` (LightBurn conversion warnings; the texts come from `doc.export_lbrn2`). LightBurn files are laid out for the document's `field_mm` |
| POST | `/api/document/upload` | `?format=openetch\|lbrn2\|svg\|dxf\|image` with the raw file as the body (at most 50 MB, else **413**) → runs on a blocking thread with origin `ui`, answered like `/api/execute` (a refused file is **200** `{ "ok": false, "error" }`; the result's `created` ids let the UI select what was imported). `openetch`: `doc.load` (replace only). `lbrn2`: `doc.import_lbrn2`, `mode=replace\|merge` (default replace), `field_mm=<mm>` (finite; default: see "Field size"). Both take `name=<file name>`: after a replacing upload downloads are named after it until the document is saved as a project (a merging LightBurn upload names its group after it). `svg`: `art.import_svg`; `dxf`: `art.import_dxf`; `image`: `art.import_image` with the file base64-encoded and `mime` from the query or sniffed from the first bytes (PNG, JPEG, GIF, BMP, WebP, TIFF; else 400). These three always add to the document (`mode` must be `merge` or absent) and take optional `layer=<id>`, `name=<text>` and `width_mm=<mm>`. Text formats must be UTF-8 (a BOM is dropped). Unknown query parameters, formats or modes, and options that do not apply to the format, are 400 |
| WS | `/api/events` | server → client JSON events (below) |
| * | `/mcp` | MCP Streamable HTTP endpoint |

`Run` = `{ id, state, origin, revision, stale, summary, stats, packet_count, created_unix_s, error?, report? }`; `summary` reads like `12 polylines on "Fill", "Outline", 340.2 mm marked, about 8.1 s` (layer names, each once, in burn order).
`state` is `"pending"` → `"approved"` → (`"awaiting_confirm"` →) `"running"` → `"finished"` \| `"failed"` \|
`"aborted"` (stopped while streaming), or `"cancelled"` (stopped before streaming, stale, or physical input not
pressed in time). `stale` is true for a pending run whose document has changed since; it cannot be approved.

Events on `/api/events`:

- `{ "type": "document_changed", "revision": n, "origin": Origin }` (after an edit, undo or redo; a client that fell
  behind gets `revision: -1, origin: null` and should refetch everything)
- `{ "type": "machine", "event": "framing" | "frame_stopped" | "run_approved" | "awaiting_confirm" |
  "run_started" | "run_finished" | "stopped" | "error" | "lagged" | "lightburn", "detail"?: "..." }` (refetch
  `GET /api/machine`; after `lightburn` also `GET /api/lightburn`)
- `{ "type": "run_pending", "run_id", "origin", "summary" }`
- `{ "type": "run_finished", "id", "ok": bool, "state", "error": string | null }`
- `{ "type": "approval_locked", "until_unix_s": n }` (too many wrong PINs; approvals locked until then)
- `{ "type": "project_saved", "name", "revision" }` (refetch `GET /api/project`)
- `{ "type": "device_changed", "id": "<device id>" }` (another device was made active; refetch `GET /api/devices` and
  `GET /api/machine`) and `{ "type": "devices_changed" }` (devices imported or removed; refetch `GET /api/devices`)
- `{ "type": "history_changed", "marker": n, "steps": n, "revision": n }` (after every edit, undo, redo, goto or step
  delete, right after `document_changed`; refetch `GET /api/history`)

**History timeline** (`commands::Session`, like Fusion 360's design timeline). Every successful edit (one execute or
one batch) is a step `{ index, revision, command, invocations: [{id, params}], origin, summary, state, error? }`:
`index` is 1-based, `command` the edit command ids joined with ", ", `invocations` the edit invocations as they ran
(queries in a batch are not recorded), `state` is `"active"` (applied), `"rolled_back"` (after the marker) or
`"failed"` (its replay failed; `error` says why; it changes nothing and is shown red). The **marker** is the index of
the last applied step (0 = none); the document is always the snapshot at the marker, so moving it is instant.

- `goto(m)` (`/api/history/goto`: a click or drag on the timeline) applies steps `1..=m` and rolls back the rest,
  deleting nothing. Undo/redo (`/api/undo`, `/api/redo`: Ctrl+Z, Ctrl+Y) move the marker over one step.
- What an edit made while rolled back does depends on what moved the marker, reported as `marker_moved_by`:
  - `"undo"` (LightBurn): the steps undone with undo/redo (`undone_steps`, right after the marker) are **discarded**
    and the edit takes their place, so redo or "go to end" never brings them back.
  - `"timeline"` (Fusion): the edit is **inserted** right after the marker; every later step is then **replayed**
    from its recorded invocations so its snapshot is current, and stays rolled back until the marker moves past it
    (drag the marker back to the end). A later step that no longer applies (it edited an object an inserted or
    deleted step removed) is kept as `failed` and skipped by undo/redo.
  - `"edit"`: the marker is at the end (nothing to discard or insert before).

  Steps rolled back on the timeline and then undone further with Ctrl+Z: only the Ctrl+Z ones are discarded, the
  rest are replayed. After an edit that leaves steps after the marker, the next edit is inserted (`"timeline"`).
- An edit that replaces the document (`doc.new`, `doc.load` including project open and `.openetch` uploads, a
  replacing `doc.import_lbrn2`) starts a **fresh timeline**: every earlier step is dropped and the replaced document
  becomes the start, so that one step can be undone but nothing of the old document is replayed into the new one.
- `delete(i)` removes step `i` and replays the steps after it; a marker after it moves back by one.
- Ids stay stable across replays: each step remembers the document's `next_id` when it first ran and replays from the
  same value, and a newly recorded step starts above every id the timeline has ever allocated, so an inserted step
  never takes an id a later step creates or uses.
- At most 500 steps are kept: older applied steps are folded into the starting document (with nothing applied, the
  newest rolled-back steps are dropped instead).
- The revision always increases (goto, undo, redo and delete each make a new revision), so stale-run checks and
  preview caching keep working.

**Sub-layers** (LightBurn's sub-layer tabs). `Layer.sub_layers` are further `CutSettings` run over the layer's
objects; a sub-layer's `name` is LightBurn's `subname`, and `cleanup: {every_n_passes}` makes it a cleanup pass.
Burn order per layer (`plan::layer_schedule`): each main pass, followed by every cleanup sub-layer due after it,
then the other sub-layers in list order, each `passes` times. A cleanup runs after main passes n, 2n, ... and after
the last main pass when no cleanup just ran, so a layer always ends cleaned and an interval of at least the main
passes runs it once, after them, like an ordinary sub-layer; each run makes its own `passes`, and its hatch angle
keeps adding `angle_increment_deg` across runs. This follows LightBurn's 3D Sliced image-mode cleanup ("Enable
cleanup pass" adds a Cleanup sub-layer that runs after every "Clean after # of passes" passes, docs 2.0/2.1 Galvo
Cut Settings); LightBurn has no cleanup for vector layers, where its sub-layers simply run after the main passes,
and "Clean whole image" (cleaning only recent passes) has no meaning without image slicing and is not modelled.
Commands (indices are 0-based positions in `sub_layers`; validation and limits as `layer.update`, at most 64
sub-layers; each one undo step): `layer.sub_add {layer, settings?, name?, cleanup?, index?}` (a copy of the main
settings with the partial `settings` merged in, like LightBurn's add; returns `index`), `layer.sub_update {layer,
index, settings?, name?, cleanup?}` (partial merge; `null` clears the name or the cleanup), `layer.sub_delete
{layer, index}`, `layer.sub_move {layer, from, to}`, `layer.sub_duplicate {layer, index}` (the copy goes right after
it; returns its `index`). Kept data of other software per sub-layer (`extras["<format>.subN.*"]`, N 1-based) moves
with its sub-layer on add, delete, move and duplicate. The web UI's "Add cleanup pass" uses LightBurn-like defaults:
the same mode and interval, one fast low-energy pass with the hatch turned 45°, cleaning after the main passes.

**Ids and arranging.** Every command that takes object `ids` also takes group ids, which stand for all the group's
members (nested groups included). `object.duplicate` and `art.array` copy a fully selected group as a new group;
`object.delete` removes groups left empty. `object.align {ids, reference?, x?, y?, mode?}` follows LightBurn: the
default reference is the **last** id (the UI passes the selection in the order objects were added to it), which stays
put; other references are `first`, `selection`, `object {id}`, `stock` and `field`; a group aligns as one object by
its bounds; top is +Y. "Centre on stock" is `object.align` with `reference: {kind: "stock"}`, `x`/`y` `center`,
`mode: "group"`. `object.distribute {ids, axis, by: centers|gaps|spacing, spacing_mm?}` keeps the outermost objects
for centres and gaps. `object.move {ids, from_point, to_point}` is a snapped drag. `stock.move {center? | translate?,
rotation_deg? | rotate_by_deg?}` moves the stock only.

**Projects.** Named projects are `<projects-dir>/<name>.openetch` (`--projects-dir`, default the OS data dir +
`openetch/projects`, e.g. `%APPDATA%\openetch\projects`, `~/Library/Application Support/openetch/projects`,
`~/.local/share/openetch/projects`; created on the first save). A name is 1-100 characters of letters, digits, space,
`-`, `_`, `.`, `(` and `)`, with no leading dot, no `..`, no leading or trailing space, and not a Windows device name
in any case or with any extension (`con`, `prn`, `aux`, `nul`, `com0`-`com9`, `lpt0`-`lpt9`, and `com¹`-`com³`,
`lpt¹`-`lpt³` with superscript digits). Names that differ only in case are the same project, as on Windows and macOS:
a save that would replace a differently-cased file needs `overwrite` like any other existing project (and the file
is then renamed to the new spelling, also on Linux), and open finds a project by another spelling when exactly one
matches. Every file is also resolved against the canonical projects directory and refused unless it stays inside
it, so links pointing elsewhere are neither read nor written through. Saves go to a fresh temporary file that is
renamed over the old one. The engine tracks the current project (name, saved revision) in memory: save and open set
it; `doc.new` and `doc.load` (including an `.openetch` upload) clear it, and so does a replacing LightBurn import
(`doc.import_lbrn2` with `mode` `replace`, the default), while a merging import keeps it. After `doc.load` (other than
opening a project) or a replacing import the document is unsaved, because no project holds it; only `doc.new` leaves
a clean document. The document is dirty whenever its revision differs from the last save, open or `doc.new` (undoing
back to the saved state still counts as a change). Document commands from `/api/execute`, `/api/batch`, uploads and the MCP
`execute` / `batch` tools run on a blocking thread, so a large import does not stall the server.

**Field size.** The engine starts with a document laid out for the active device (a galvo's `field_mm`, from the
machine profile when it drives the machine; a gantry's `workspace_mm` and origin corner), and `doc.new` without a
size starts at the active device's again (the engine fills it in; so New never keeps an imported 150 mm).
`doc.load` keeps it when the new document has none and imports fill it in, so it stays set; when it is ever missing,
`doc.export_lbrn2` called without `field_mm` gets the profile's. `doc.import_lbrn2` without `field_mm` or `workspace_mm`
(uploads, `/api/execute`, MCP) gets, in order: the layout of the OpenEtch device the file's root `DeviceName` names
(display name, LightBurn name or id, ignoring case; LightBurn 2.x writes the device's display name, e.g. `JCZFiber
(Unknown)`), else the field (`Width`) of a LightBurn device of that name in `prefs.ini` that was not imported, else the
active device's, else `document.field_mm`, else the profile's. A galvo device's layout is its LightBurn `Width` (the
field LightBurn laid the file out for: 153.8 on the B6, whose profile keeps EzCad's 153.75), else its field; a
gantry's is `workspace_mm` and `origin`. The result reports `field_mm`, `workspace_mm`, `origin`, `field_source`
(`"file device JCZFiber (Unknown)"`, `"file device JCZFiber"` for the prefs step, `"active device <name>"`,
`"document"`, `"machine"` or `"caller"`) and `device` (the id, when a device was used). The engine writes the chosen size into the recorded invocation, so replays never depend on
LightBurn's prefs or the active device. A LightBurn import remembers
the field size it was read with, so an export puts every shape back where LightBurn had it: a replacing import sets
`document.field_mm` to it, and a merging one (which keeps the document's) stores it in the document's
`extras["lbrn2._field_mm"]`. `doc.export_lbrn2` (and the LightBurn download and `project_export_lbrn2`) uses, in
order: its `field_mm` parameter, `extras["lbrn2._field_mm"]`, `document.field_mm`.

**Access.** Every request's `Host` must be a loopback name, an IP literal or an `--allow-host` name, and an `Origin`
header, when present, must be loopback, allowed or same-host (DNS-rebinding and cross-site protection). On loopback
no token is needed. Off loopback (`--listen 0.0.0.0:7878` on the Pi) a bearer token is generated (or given with
`--token`) and printed in the startup URL: `/api/*` and `/mcp` need `Authorization: Bearer <token>`; only
`GET /api/events` (the WebSocket, where browsers cannot set headers) also accepts `?token=`. Static files and
`POST /api/machine/stop` need no token.

Default listen address `127.0.0.1:7878`. Other options: `--profile`, `--web-dir`, `--usb`, `--token`,
`--allow-host`, `--config-dir`, `--confirm-input`, `--projects-dir`, `--lightburn-prefs <path>` (default the OS
location, a missing file is fine), `--lightburn-device <name>` (the LightBurn device `GET /api/lightburn` and the
machine status compare with), `--device <id or name>` (the OpenEtch device for this run, overriding the stored active
one without changing it); `openetch devices list` and `openetch devices import <file> [--add all|<name>]` (both
`[--config-dir P] [--profile P]`; see "Devices"); `openetch set-pin [--reset] [--config-dir <path>]` sets, changes
or deletes the operator PIN (restart the engine afterwards); `openetch profile from-lightburn [--prefs P] [--device
NAME] [--base PROFILE] --out FILE [--force]` prints the diff table and writes a profile TOML for a LightBurn device
(base default `fixtures/b6-mopa-60w.toml`, else the generic profile; refuses to replace `--out` without `--force`).

## MCP tools

`list_commands`, `execute`, `batch`, `inspect_document`, `render_preview` (PNG), `undo`, `redo`, `history_list`
(`{marker, steps}`, as `GET /api/history`), `history_goto {marker}`, plus separate machine tools: `machine_status`,
`frame_start`, `frame_stop`, `job_request_run`, `machine_stop`, and project tools: `project_list` (`{projects, current}`), `project_save {name, overwrite?}` (replacing an existing project other than
the current one needs `overwrite: true`, default false), `project_open {name}` and
`project_export_lbrn2 {name, overwrite?}` (writes `<projects-dir>/<name>.lbrn2`; replacing an existing one, also one
whose name differs only in case, needs `overwrite: true`, default false; returns `{path, warnings, field_mm}`), and
`lightburn_devices` (`GET /api/lightburn` plus `machine`: the `lightburn` part of `GET /api/machine`) and
`devices_list` (`GET /api/devices`, read-only: activating, importing and removing devices change the machine, so they
are UI-only, and the server instructions say so; design-only devices refuse `frame_start` and `job_request_run`). The
`doc.import_lbrn2` description and the server instructions state the field-size rule.
Project tools take names, never paths.
File contents go through `execute` with `doc.save`, `doc.load`, `doc.import_lbrn2` and `doc.export_lbrn2`. The MCP
surface is generated from the same command registry the UI uses; machine tools are separate so clients can approve
them separately. No
MCP tool can approve a run or touch the operator PIN, and the server instructions say agents cannot and must not.

## File formats

- **`.openetch`** (native project): UTF-8 JSON `{"format": "openetch", "version": 1, "document": <doc::Document>}`
  (`doc::file`). Readers refuse other formats (XML is pointed to the LightBurn import) and newer versions, accept a
  UTF-8 BOM, refuse duplicate ids and raise `next_id` past existing ids. `Document.field_mm` is the galvo field the
  document was laid out for; `Document.workspace_mm` (`[w, h]`, optional, left out when absent) is a gantry's bed
  (then `field_mm` is the smaller side, and `extras["lbrn2._origin"]` the device's origin corner). `Document.groups` (`{id, name, parent?}`, nested through `parent`) and `Object.group`
  (the innermost group) hold LightBurn-style groups; a group with no member objects is removed
  (`Document::prune_groups`, also on load). `Geometry::Image {width_mm, height_mm, mime, data_base64}` is a bitmap
  centred on the local origin; jobs skip images with the plan warning "image engraving is not supported yet".
  `extras` on the document, layers, objects and groups keep data from other software that OpenEtch does not model
  (`lbrn2.*`) so exports round-trip. All of these are optional, so files from older builds load (still version 1).
  Commands: `doc.new`, `doc.load {content}`, `doc.save` (Query, returns `{content, revision}`).
- **`.lbrn2`** (LightBurn project, `crates/lbrn`): XML; mm, y up, origin at the lower-left of LightBurn's workspace,
  so field frame = LightBurn - `field_mm / 2`. The file does not record the field size it was laid out for, so
  import and export take `field_mm` (import default: a LightBurn device's field, else the document's; export default the field the last import used,
  see "Field size"); a wrong value shifts everything by half the difference. CutSettings become layers (power, speed, frequency Hz to kHz, QPulseWidth ns, hatch, passes,
  sub-layers with their `subname` as `name` and `isCleanup` as `cleanup`, whose `numPasses` is then
  `every_n_passes` ("Clean after # of passes"; the parent's `cleanupPass` is written while a cleanup sub-layer
  exists); everything else in `extras["lbrn2.*"]`), Rect/Ellipse/Path become objects, LightBurn groups become
  groups, Text becomes an object drawn and burned as its saved outline (BackupPath) that keeps its text attributes
  and is exported as LightBurn text again, bitmaps become images; anything else (unknown shape types, the
  thumbnail, `UIPrefs`, `Notes`, root attributes, layout) is kept in `extras`. Export writes it all back, with the
  source text of every number, transform and path list that did not change, so an unchanged import exports byte for
  byte (the env-gated `crates/lbrn/tests/real_samples.rs` checks every sample file). `.lbt` templates read the same
  way. Export keeps each layer's LightBurn index and writes the stock outline on tool layer T1. Commands:
  `doc.import_lbrn2 {content, mode?: replace|merge, field_mm?, workspace_mm?, origin?, name?}` (Edit; merge brings the
  file's objects in as one group) and `doc.export_lbrn2 {field_mm?, workspace_mm?, origin?}` (Query, returns `{content,
  warnings, field_mm, workspace_mm, origin}`); `doc.new {name?, field_mm?, workspace_mm?, origin?}`. The format notes
  are in `crates/lbrn/src/lib.rs`.
  **Gantry beds** (`lbrn::Frame::Bed`; `origin` a corner, with `workspace_mm`): LightBurn measures in mm from the
  device's origin corner, which the project root records as `MirrorX` / `MirrorY` (front-left = neither, rear-left =
  `MirrorY`, rear-right = both, front-right = `MirrorX`; orientation checked against the thumbnails of public
  LightBurn-saved projects, see `crates/lbrn/src/lib.rs`; mirroring about the bed rather than 0 is likely, unshown). Import
  uses the file's own flags: field = S · (LightBurn - [W/2, H/2]), S negating the mirrored axes (a shape saved in a
  mirrored frame gets a mirrored transform, so it looks as LightBurn showed it); export applies the inverse for the
  target origin and writes its flags, so a file saved for another origin is reoriented as LightBurn does since 1.3.
  Unverified against a file LightBurn saved for a rear-left device. The bed and origin are remembered like the
  galvo field (`extras["lbrn2._workspace_mm"]` for merges, `extras["lbrn2._origin"]`); an export's default origin is
  the document's, else `bottom_left` for a document with `workspace_mm`, else `center` (galvo). Galvo imports never
  apply the file's mirror flags (galvos use absolute coordinates; a set flag is reported).
- **`.dxf`, images**: `art.import_dxf {content, layer?, name?, width_mm?, split?, keep_position?}` (lines,
  polylines with bulges, circles, arcs, ellipses, splines, block references; units from `$INSUNITS`, else mm with a
  warning; native builds only; `split` defaults to true: one object per entity, all in one group named `name` (default
  "Imported DXF"), as LightBurn imports a DXF, so dropped and uploaded DXF files arrive that way; `split: false` makes one
  combined object) and `art.import_image {data_base64, mime, width_mm?, layer?, name?}` (PNG, JPEG, GIF,
  BMP, WebP; size from the file's resolution, else 254 dpi, fitted to 80 % of the field). Groups: `group.create {ids,
  name?}`, `group.ungroup {group}` (one level), `group.rename {group, name}`.
- **`.clb`** (LightBurn material library): read by `lbrn::parse_library` into layer settings.

## LightBurn devices

LightBurn keeps its devices in `prefs.ini`, one JSON object with all of LightBurn's settings: Windows
`%LOCALAPPDATA%\LightBurn\prefs.ini`, macOS `~/Library/Preferences/LightBurn/prefs.ini` (then `~/Library/Application
Support/LightBurn`, unverified), Linux `~/.local/share/LightBurn/prefs.ini` then `~/.config/LightBurn/prefs.ini`
(**unverified**: LightBurn's documentation does not say; a LightBurn forum answer names `~/.local/share/LightBurn`,
which is also Qt's app-data location; File > Open Prefs Folder in LightBurn shows the real one).

- **Privacy.** The file also holds unrelated settings and secrets (e.g. `RestPendantSecret`). `machine::parse_devices`
  (pure) reads only `DeviceList` and `DefaultDevice`, and inside each device only typed keys (name, display name,
  type, width/height, mirror, and a fixed subset of `Settings`); every other key is skipped by the deserializer, the
  correction file is cut to its file name, and errors give a line and column, never content. The engine reads the
  file into memory only to parse it (again when its size or mtime changes), never writes it, stores only the parsed
  devices (an upload, `lightburn-devices.json`; imported devices, `devices/<id>.toml`), returns only typed device
  fields (`GET /api/lightburn`: `{name, display_name, field_mm, width_mm, height_mm}`; `GET /api/devices/{id}`: the
  Device Settings listed under "Devices")
  and the profile diffs, and logs only the path and device count. An uploaded list wins over the local file until
  it is deleted. Tests use the synthetic `crates/machine/tests/fixtures/lightburn-prefs.json`; real files are read in
  place by the env-gated `crates/machine/tests/lightburn.rs` (`OPENETCH_LIGHTBURN_PREFS`, `;`-separated).
- **Lookup.** `find_device` matches `Name` or `DisplayName`, ignoring case; `.lbrn2` files carry the display name.
  The device `GET /api/lightburn` reports as the default and the machine status compares with is
  `--lightburn-device`, else LightBurn's `DefaultDevice` (else the only device). LightBurn imports use OpenEtch's
  devices first (see "Devices" and "Field size"); `/api/lightburn` stays for compatibility, the UI uses
  `/api/devices`.
- **Profile from a device** (`machine::profile_from_lightburn(device, base) -> (profile, diffs)`; table in
  `crates/machine/src/lightburn.rs`): field (`Width`; within 0.05 mm of the base keeps the base, since LightBurn shows
  the field to 0.1 mm: the B6's 153.8 is EzCad's `FIELDSIZE=153.75`; width != height is reported), `swap_xy`,
  `negate_x/y`, `cor_file` (file name), laser on/off/polygon/end delays, jump speed and delays, jump distance limit,
  `mark_speed.max_mm_s` (`MaxSpeed`, 7000 on the B6: only lowers it), frequency range (only narrowed),
  `pulse_width_enabled` (`Laser_QPulseWidthEnabled`), `open_mo_delay_us` (`Laser_OpenMODelay`, default 8000),
  `start_mark_input_bit` (`StartMarkPortBit`, 14 on the B6, also read from markcfg `STARTMARKPORT`),
  `red_light_speed_mm_s` (`FrameSpeed`). The last four profile fields are new and informational (the run prelude
  does not wait for the MO delay yet; `plan` sends pulse widths regardless; `--confirm-input` is what waits for an
  input). Hard limits are never raised; every change is validated one at a time, and clamps, refusals and settings
  OpenEtch does not model (`FieldAngle`, `FieldOffset`, galvo scale/skew/trapezoid/bulge, a red-light bit other than
  8, ...) are reported as diffs. The result always passes `MachineProfile::validate`.
- **Axis swap (UNVERIFIED on hardware).** The owner's B6 works from LightBurn with `Galvo_1_is_X = false` and from
  EzCad with markcfg `GALVOX = 1`. `from_markcfg` maps `GALVOX = 1` to no swap, so the LightBurn import maps
  `swap_xy = Galvo_1_is_X` (false = no swap) and both agree on this machine: the two programs label the galvo
  channels the opposite way round. Then LightBurn's galvo 2 carries field X when not swapped, so `negate_x` comes
  from `Galvo_2_Sign` (galvo 1 when swapped), XOR `MirrorX` (likewise Y). If a frame comes out mirrored across the
  diagonal, both mappings are wrong together; the signs are 1 on the B6, so their assignment is untested.

## Devices

LightBurn-style devices (`machine::device`, the registry in `crates/engine/src/devices.rs`), so a user can bring any
laser profile from LightBurn. Spec and evidence: LightBurn's docs (Devices window, Device Settings, Coordinates and
Origin), the public WAZER `.lbdev`, xTool/Creality `.lbdev` posts and the owner's `prefs.ini` files.

- **Device** = `{id (slug of the display name), name (LightBurn Name: the controller), display_name, source:
  builtin | lightburn-prefs | lbdev | manual, kind: jcz_galvo | gantry | galvo_other | unknown, lightburn_type
  (LightBurn Type: the connection, e.g. LibUSB), workspace_mm [w, h], origin, controllable, profile (JCZ galvos),
  lightburn (the typed LightBurn settings it was made from)}`. Only a JCZ (BJJCZ LMC, EzCad2-family, LightBurn's
  "JCZFiber") galvo is **controllable**; every other device is **design-only**: its workspace drives the canvas, new
  documents and LightBurn import/export, and previews, runs and framing are refused (422, with the reason).
- **Family** (LightBurn's `Name` is an open set, never matched against a closed list): a galvo when `Type` is
  `LibUSB`, `Name` contains Fiber or EZCad, or any `Galvo_*` setting exists; JCZ when such a `Name` contains JCZ
  (BSLFiber and EZCad3 are `galvo_other`); `unknown` for "No Machine"; everything else is a gantry (GCode/DSP).
- **Workspace and origin**: a galvo's field (`Width`; for a JCZ galvo the profile's field, e.g. 153.75 on the B6) with
  the centre origin; a gantry's `Width x Height` bed and the corner its `MirrorX`/`MirrorY` name (see "File
  formats"). Devices without a usable size are skipped on import, with the reason.
- **Profile** of a JCZ galvo: `profile_from_lightburn` over the built-in B6 profile when the device is the same lens
  (width within 0.05 mm, same correction file or none), else over the generic JCZ galvo base
  (`fixtures/generic-jcz-galvo.toml`, `MachineProfile::generic_jcz_galvo`: 20..4000 kHz, mark speed up to 5000 mm/s,
  travel up to 3000 mm/s, no pulse width unless LightBurn enables it; hard limits are never raised by an import).
  `GET /api/devices/{id}` recomputes the diffs against that base. The correction file (a name only) is looked up in
  `<config-dir>/devices/`, then next to the built-in profile; a missing one is a warning on activation.
- **Registry**: `<config-dir>/devices/index.json` (`{format: "openetch-devices", version: 1, active, order}`) and one
  `<id>.toml` per imported device (`{format: "openetch-device", version: 1, ...}`, re-checked on load: a valid id,
  workspace and profile; `controllable` is recomputed). The built-in device ("ComMarker B6 MOPA 60W (EzCad)", the
  `--profile` fixture) is never stored. **First run** (no index): the devices of the auto-detected LightBurn
  `prefs.ini` are imported (read only) and LightBurn's default device becomes active if it is a controllable JCZ
  galvo, else the built-in device. Broken device files are skipped and reported at startup.
- **Import** (`POST /api/devices/import`, `openetch devices import`): a `prefs.ini` or a `.lbdev` (plain JSON
  `{"DeviceList": [...]}`, the shape of `prefs.ini`'s list, without `DefaultDevice`; `machine::parse_lbdev`), through
  the same typed parser: `.lbdev` files also hold macros, G-code, addresses and paths, none of which is read; the
  material library path is cut to its file name. `.lbzip` bundles and `.lbvendor` files are not read (their layout is
  undocumented). A device with the same GUID-free identity (display name, controller, connection, LightBurn size) as
  an imported one is updated in place.
- **Active device**: stored in the index; `--device` overrides it for one run. Activation (UI only) is refused while
  a run is approved, awaiting confirmation or running, or while framing; it claims the machine like a run, cancels
  pending runs, swaps the profile (`hw::Machine::swap_config` under the controller lock: a new
  `MachineConfig { profile, cor, target, generation }`, the simulator gets a fresh `SimLink`, a USB board is closed and
  initialised again) and emits `device_changed`. A run remembers the configuration generation it was planned for;
  approval refuses one of another generation, and a run planned while the device changed is refused when stored. A
  design-only device leaves the machine profile as it was (nothing can run).
- **Settings shown** (read-only, `GET /api/devices/{id}` `settings`): name, controller, connection, work area,
  origin, auto-home, bounds check, job origin, library file, preview speeds; galvo: field, offset, angle, COR file,
  galvo axis/sign/shape, frame/max/jump speeds, jump delays and limit, laser timing, laser/fiber type, frequency range,
  Q-pulse/PWM, open MO delay, red-light and start-mark bits; GCode: baud rate, network port, S-value max, air assist,
  Z/U axis, transfer mode, G-code flavor. DSP-specific keys are not read (their names are unconfirmed).

## Licensing and provenance

Code ported from MIT/Apache projects keeps its notice in the file header and is listed in `ATTRIBUTION.md`. Never copy
from GPL/SSPL/unlicensed sources (Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader).
