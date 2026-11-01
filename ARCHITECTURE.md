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
| `doc` | Document model: stock, layers with MOPA cut settings, objects (kurbo paths + affine transforms), guides, output curve tolerance. **Pure data + geometry helpers.** | kurbo, geom (polygon outlines) | no |
| `lbrn` | LightBurn `.lbrn2` projects and `.clb` material libraries: parse, write, convert to and from `doc` for a galvo field or a gantry bed (`lbrn::Frame`, see "File formats"). **Pure.** | doc, kurbo, roxmltree | no |
| `geom` | 2D geometry on kurbo paths in mm: polygon booleans, weld, cut, offsets (`i_overlay`), curve-preserving path tools (break apart, join, close, optimize, arc fitting and smoothing, fillet on any corner, trim, extend), node editing, generators (polygons, convex hull, tangent circles), warp/deform, path walking, bounding-box and true-shape nesting. **Pure.** | kurbo, i_overlay | no (wasm-ok) |
| `text` | Text like LightBurn (see "Text"): fonts (installed TrueType/OpenType via fontdb + harfrust shaping + skrifa outlines, the bundled single-line Hershey fonts, AutoCAD `.shx`, EzCad `.jsf`), layout, variable text, barcodes (rxing). **Pure** except font discovery and the clock (feature `system`). | doc, geom, kurbo | `system` feature only (wasm-ok without) |
| `commands` | Command registry: every change to a document goes through a registered command with a JSON Schema. Session with a Fusion-style timeline (see "History timeline"). | doc, geom, text | no |
| `plan` | Document → job: flatten paths, hatch fills, Offset Fill and kerf offsets (through `geom`), raster images (decode, adjust, dither; see "Image engraving"), order travel, encode to LMC list packets for a `MachineProfile`. Also simulation (decode packets to a preview). | doc, lmc, geom, image | no |
| `machine` | Machine profile (TOML, importable from EzCad `markcfg7` and from a LightBurn device, see "LightBurn devices"), devices (`machine::device`: LightBurn-style device model, family, workspace, origin), the `Link` transport trait, `SimLink`, `UsbLink` (nusb, feature `usb`), `Controller` (init, .cor upload, light framing, run, abort). | lmc, corfile | `usb` feature only |
| `fileio` | File formats and image tools (see "Files and image tools"): SVG/DXF/PDF/AI export and print, PDF/AI/EPS/HPGL/Gerber/Excellon/EzCad/legacy `.lbrn` import, 3D models to depth maps, TIFF/TGA, zip, trace, rasterise, masks. **Pure** except PDF reading (native only). | doc, geom, kurbo, image, roxmltree, miniz_oxide, lopdf | no (wasm-ok without PDF reading) |
| `engine` | Binary. axum HTTP + WebSocket API for the web UI, MCP server (rmcp), run approval, serves `web/dist`. | all | yes |
| `web/` | Fusion-style UI: React + TypeScript + Vite. Talks to the engine over HTTP + WebSocket. | – | – |

## Coordinate frames and units

- All public APIs use **millimetres** and **degrees**.
- **Field frame**: origin at the centre of the galvo field, +X to the operator's right, +Y away from the operator
  (toward the column), matching LightBurn's galvo frame. Objects, stock and jobs are all expressed in the field frame.
- **Gantry devices** (design-only, see "Devices") use the same frame: origin at the centre of the bed
  (`Document.workspace_mm` `[w, h]`), +Y away from the operator (toward the rear).
- Galvo units are only used inside `lmc`/`plan`/`machine`: `0x0000..=0xFFFF` across `field_mm`, centre `0x8000`.
  A **non-square field** (LightBurn's separate field Width and Height; `MachineProfile.field_height_mm`,
  `FieldTransform.field_height_mm`, `plan::Target.field_height_mm`, all `None` for a square field) maps field X over
  `field_mm` and field Y over the height, each axis onto the full word range (the swap keeps each field axis with its
  own size); Y is bounded by `±height / 2`. Speeds convert with the larger side (`FieldTransform::speed_field_mm`), so
  neither axis runs faster than asked. The device's workspace is then `[field_mm, height]`, and so is the
  document's: a document made for such a galvo (`state::new_document_for`, `doc.new {workspace_mm, origin:
  "center"}`) has `workspace_mm [width, height]`, `field_mm` the width and `extras` `lbrn2._origin = "center"`, so
  the canvas draws the real field (`Document::workspace`); LightBurn files of such a field convert from its
  lower-left corner on both axes (`file.rs` `Layout::Field`). `GET /api/machine` reports `field_height_mm`.
- Axis swap / negate from the machine profile and lens correction are applied in `plan`/`machine` when converting
  field mm to galvo units, never in `doc`. Both use the one function pair `lmc::units::field_to_galvo` /
  `galvo_to_field` with a `FieldTransform { field_mm, swap_xy, negate_x, negate_y, calibration }`: points with
  `|coord| >= field_mm / 2` are refused, never clamped; then the lens correction (`lmc::Calibration`: LightBurn's field
  offset X/Y, field angle and per-axis scale, bulge, skew, trapezoid; neutral by default; a corrected point that no
  longer encodes is refused); then negate in the field frame, then swap. So the red-pointer frame and the burn always
  land in the same place. The correction model is OpenEtch's own (see `crates/lmc/src/calibration.rs`): LightBurn
  documents what each value is for but not its formula, so imported values act in the same direction with the same
  neutral points, but their exact effect is unverified against LightBurn.
- **Red dot alignment** (`MachineProfile.red_dot`: offset X/Y, scale X/Y, rest position) applies to framing only,
  before the lens correction: the pointer diode sits off the beam axis. Framing parks the dot at the rest position when
  it stops.

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
- **Simulator test PIN** (`--sim-pin <6-12 digits>`, for testing the approval flow without a laser). It becomes the
  operator PIN for that engine run, held in memory as a salted hash and never written to disk (the PIN file is
  neither read, watched nor touched). It is refused at startup with `--usb`, and again after the machine is built
  unless it is the simulator (`LinkKind::Simulated`); a release build also refuses it unless
  `OPENETCH_ALLOW_SIM_PIN=1` is set. `approve` re-checks on every attempt and answers `sim_pin_real_link` (403) when
  the machine is not the simulator, before the PIN is even looked at. The engine logs a loud warning, and
  `GET /api/machine` reports `approval.sim_pin: true` (the UI shows a "SIMULATOR TEST PIN" badge). Why it cannot
  weaken real-laser safety: the link kind is fixed when the engine starts (device changes swap the profile, never
  the link), so an engine that drives the laser can never hold a simulator PIN, and an engine that holds one can
  only arm runs on `CountingSimLink`, which never leaves the process. Everything else about approval (lockouts,
  UI-only origin, Require Framing, checklist, physical confirmation) applies unchanged. Putting a PIN on the command
  line only needs the same OS access that could already restart the engine or open USB, which the PIN never held
  against (see above).
- **Physical confirmation (optional).** With `--confirm-input <bit 0-15>` an approved run waits (state
  `awaiting_confirm`) up to 30 s, polling `ReadPort` (reply word 1, as galvoplotter reads it), until that input is
  seen released and then pressed for 3 reads in a row; a stuck, held or inverted input therefore never confirms. Stop
  cancels the wait; a timeout cancels the run. The B6's `markcfg` has `STARTMARKPORT=14`, which suggests a foot pedal
  on bit 14 (unverified, as is the active-high polarity).
- **Repeat, run continuously, start input.** A run may ask for `repeat` iterations or `continuous` (until stopped,
  Stop At or "end after the current run"); one PIN approval covers them. Every iteration gets its own arming token in
  `approve` (the only module that makes one), checks the stop epoch first, and with `start_input` waits for the
  profile's start-marking input (foot pedal) to be released and pressed (`full_pulse`, the default; or merely pressed)
  before it starts; a stop cancels the wait. An active-low input (`MachineProfile.start_mark_active_low`, from
  LightBurn's `StartMarkBitHigh = false` and written back to it) is pressed while its bit reads 0. Stop At refuses approvals once the job counter reaches it.
- **Pause / resume.** `POST /api/machine/pause` is not gated (it sends `StopList`, galvoplotter's and meerk40t's
  pause, at the next packet or poll); `POST /api/machine/resume` (`RestartList`) continues an approved run and is
  UI-only (MCP headers or a non-`ui` origin are refused). Stop ends a paused run like a running one. A pause pressed
  while no list streams (between iterations, while waiting for the start input or the physical confirmation) holds
  the next iteration before its MO opens until resume or stop; a pause left over when a job ends is dropped at the
  next approval. That the B6 holds
  its place and MO state while paused is unverified on hardware.
- **Stop-marking input.** With `stop_mark_input_bit` in the profile, a running job reads the inputs at every status
  poll and aborts (`ControllerError::StopInput`) when that bit is set.
- **IO ports** (`MachineProfile.io`, LightBurn's galvo IO Ports; all unverified on hardware): `door_protect` (an
  input with its level, `active_low`): approval reads it and refuses while the door is open, `Controller::run_parts`
  reads it before anything that can emit, and a running job aborts (`ControllerError::DoorOpen`) when it opens;
  `start_framing` (input): the engine reads it every 150 ms while the board is idle and starts the last frame
  request (else the job's bounds) when it is pressed after being seen released; `busy_light`, `ready_light`
  (outputs held during a job: they ride along with every `WritePort` of the prelude and of the list, `plan::Target.
  port_outputs`; `check_job` allows exactly those extra bits), `done_marking` (pulsed `done_pulse_ms` after a job,
  with the MO closed), `red_dot_always_on` (the red pointer in the idle output word, written after init, a frame
  stop, a run and the abort sequence; never during a job). No output may be the laser-enable, red-pointer or focus
  bit, and none is ever written together with the laser-enable bit outside a job.
- **Require Framing Before Start, Job Checklist** (per device, `PUT /api/prefs`, UI only): with Require Framing on,
  approval is refused until a frame was started on the document revision the run was planned from; with the
  checklist on, the approval must tick every item (`checklist: [true, ...]`). Both are checked before the machine is
  claimed.
- **Extension axis** (Z lift or rotary, `MachineProfile.axis`, LMC `MoveAxisTo(position, invert)` after meerk40t):
  moves never emit; a run moves the axis between its parts with the MO closed and opens it again afterwards (each
  part's list waits the open-MO delay); `POST /api/machine/axis` is UI-only. The B6's lift is not on the board
  (`axis` unset). Unverified on hardware.
- **Stop is never gated and never waits behind a run.** `POST /api/machine/stop` needs no token. It bumps a stop epoch
  (`AtomicU64`) and requests an abort before it waits for the controller; a run captures the epoch when it claims the
  machine and never starts streaming once it has changed, even if the stop's abort sequence reached the controller
  first. A running job notices the abort request at its next packet or status poll. If a run task panics, the engine
  sends the abort sequence (and `Controller::run` aborts from its own drop guard).
- Hard limits (max power, field bounds, mark speed) live in the machine profile and cannot be changed by command
  arguments. Layers and sub-layers may have at most 1000 passes (`commands` and `plan` both refuse more), a layer at
  most 64 sub-layers (`commands`) and 100,000 planned passes in all (`plan`, counting cleanup runs), and `plan_job`
  refuses jobs above 2,000,000 segments (an image's scan runs count, and the error names the image), images that
  would be sampled into more than 100,000,000 dots in a pass or decode to more than 64,000,000 pixels, so one edit
  cannot make planning exhaust the engine's memory. Previews are coalesced: at
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
| GET | `/api/preview` | planned job: `{ segments: [{kind: "jump"|"mark", layer, pass, from: [x,y], to: [x,y], emits, power_pct?}], passes: [{layer_id, sub_layer, repeat, segments, mark_length_mm, jump_length_mm, estimated_time_s}], stats, layers: [{layer_id, name, color, mode, output, visible, passes, segments, mark_length_mm, jump_length_mm, estimated_time_s, sub_layers: [{sub_layer, name, label, cleanup, mode, power_pct, speed_mm_s, frequency_khz, pulse_width_ns, passes, planned_passes, segments, mark_length_mm, jump_length_mm, estimated_time_s}]}], warnings }` in field mm. `pass` indexes `passes`, the job's passes in burn order (a jump belongs to the pass it travels into); `sub_layer` is 0 for a layer's main settings and n for `sub_layers[n - 1]`; `layers` lists every document layer in order (zeros for layers with nothing to mark or output off; layer times leave out the job's start-up delay), each with its main settings and sub-layers (`label` "Main", the name, "Cleanup" or "Sub-layer n"; `passes` asked for, `planned_passes` planned, and their share of the layer's totals); `power_pct` is set on marks whose power differs from their pass's (grayscale image runs); `warnings` are things the plan left out (none at present). `?layers=id1,id2` keeps only those layers' segments and entries (stats and `passes` stay the whole job's). An empty document is an empty preview; a planning error (out of field, power above the limit, too many passes/segments) is **422** |
| GET | `/api/fill_regions` | `{ layers: [{layer_id, name, color, mode, visible, output, paths: [d], object_ids: [id]}] }`: for every layer whose mode is `fill` or `fill_then_line`, one SVG path per object (its closed subpaths, field mm, +Y up, flattened at 0.05 mm) to draw with `fill-rule: evenodd`, which is exactly the area the hatch covers. Display only |
| GET | `/api/preview.svg` | rendered preview (stock, objects, toolpath; `?jumps=false` hides travel); still renders the document on a planning error |
| GET | `/api/machine` | `connected`, `simulated`, `busy`, `state`, `running`, `framing`, `field_mm`, `profile`, `device` (the active device's summary, see `GET /api/devices`), `max_power_pct`, `last_error`, `sim_counters`, `approval: { pin_set, pin_changed, locked_until_unix_s: null \| n, sim_pin (the simulator test PIN is in use, see "Safety model"), physical_confirm: null \| { input_bit, timeout_s } }`, `focus_light` (the active device can light a focus pointer: framing's `pointer: focus \| both` work), `lightburn: null \| { device, field_mm, diffs: [{field, base_value, lightburn_value, note}] }` (the active profile compared with the matched LightBurn device, see "LightBurn devices"), `pending_runs: [Run]`, `runs: [Run]` |
| GET | `/api/lightburn` | `{ available, source: "file" \| "uploaded" \| null, default_device: name \| null, devices: [{name, display_name, field_mm, width_mm, height_mm}] }` (only these fields); `default_device` is the device used when a LightBurn file names none |
| POST | `/api/lightburn/prefs` | LightBurn's `prefs.ini` as the raw body (at most 2 MB, else **413**): parses `DeviceList` in memory and stores only the parsed devices (`<config-dir>/lightburn-devices.json`), for an engine on a Pi without LightBurn; answers like `GET /api/lightburn`; no devices, not JSON or not UTF-8 is 400. Emits `machine` event `lightburn` |
| DELETE | `/api/lightburn/prefs` | forgets the uploaded devices (the local `prefs.ini` is used again, if any); answers like `GET /api/lightburn`; emits `machine` event `lightburn` |
| GET | `/api/devices` | `{ active: id, devices: [{id, name, display_name, source: "builtin" \| "lightburn-prefs" \| "lbdev" \| "manual", kind: "jcz_galvo" \| "gantry" \| "galvo_other" \| "unknown", lightburn_type, workspace_mm: [w, h], lightburn_field_mm: n \| null (galvos: the field LightBurn files are laid out for, LightBurn's Width, e.g. 153.8 on the B6 whose profile field is 153.75), origin: "center" \| "bottom_left" \| "top_left" \| "bottom_right" \| "top_right", controllable, reason: string \| null, active}] }` in list order (the built-in device first); `reason` says why a design-only device cannot be driven. See "Devices" |
| GET | `/api/devices/{id}` | the summary plus `settings` (LightBurn's Device Settings, read-only: `[{title, rows: [{label, key, value}]}]`, `key` the LightBurn key), `profile` (JCZ galvos: name, field, swap/negate, cor file, frequency range, limits, timing, `calibration` (field offset X/Y, angle, scale, bulge, skew, trapezoid), `red_dot` (offset, scale, rest), `blanking`, `red_light_port_bit`, `red_light_speed_mm_s`, `focus_light_port_bit`, `laser_enable_port_bit`, `start_mark_input_bit`, `stop_mark_input_bit`, `io` (Door Protect, Start Framing, Busy / Ready / Done Marking lights, Red Dot Always On), `inputs: [{role: start_marking \| stop_marking \| door_protect \| start_framing, bit, active_low}]`, `outputs: [{role: laser_enable \| red_light \| focus_light \| busy_light \| ready_light \| done_marking, bit}]`), `base` (the profile it was built on) and `diffs` (`[{field, base_value, lightburn_value, note}]`, the profile against that base); unknown id 404 |
| POST | `/api/devices/import` | `?add=all\|<name>` (default all) with a LightBurn `.lbdev` or `prefs.ini` as the raw body (at most 2 MB, else **413**), or `?from=lightburn` and no body: the devices of LightBurn's `prefs.ini` on the engine's computer (or the uploaded one) → `{ ok, added: [id], updated: [id], skipped: [{name, reason}] }`. Parsed with the typed, privacy-safe parser; a device with the same display name, controller, connection and size as an imported one is updated (same id). Not UTF-8, not JSON or no devices 400; `add=<name>` naming no device of the file 404. Emits `devices_changed` |
| DELETE | `/api/devices/{id}` | → `{ ok, id }`; the built-in or the active device 409; unknown 404. Emits `devices_changed` |
| POST | `/api/devices/{id}/activate` | optional `{ "origin" }` → `{ ok, active: summary, warnings: [text] }`: makes the device active (stored; it is also the one used at the next start). Refused **409** while a run is approved, awaiting confirmation or running, or while framing; MCP headers or a non-`ui` origin **403**. Cancels pending runs, swaps the machine profile (JCZ galvos; the board is initialised again, the simulator gets a fresh `SimLink`), and new documents start at the device's workspace. Emits `device_changed`. A correction file that cannot be found is a warning (output is then uncorrected) |
| POST | `/api/runs` | optional `{ "origin", "repeat"?: 1..=10000, "continuous"?: bool, "start_input"?: bool, "full_pulse"?: bool, "selection"?: [id], "from_segment"?: n }` → **201** `{ "ok": true, "id", "run": Run, "message" }`; plans, encodes and checks the document (with `selection` only those objects/groups, LightBurn's Cut Selected Graphics; with `from_segment` from that `GET /api/preview` segment on, Start Here, not combined with `selection` (400); the job counter is the run index of auto-rotating fills, and `GET /api/preview` plans with the same index; `start_input` when the profile's start- and stop-marking inputs are the same bit 400); sends nothing to the board; at most 8 pending; unknown keys 400 |
| POST | `/api/runs/{id}/approve` | `{ "pin": "<operator PIN>" }` → `{ "ok": true, "id", "state": "approved" \| "awaiting_confirm" }`; the run then starts in the background. PIN refusals are **403** `{ "ok": false, "error", "code": "pin_not_set" \| "pin_changed" \| "pin_required" \| "pin_invalid" \| "pin_locked" }`; MCP headers or a non-`ui` origin 403; unknown run 404; not pending, stale revision or another run active 409 |
| POST | `/api/machine/frame` | `{ "ids"?: [...], "mode"?: "bounds" \| "hull" \| "outline" (alias "contour"), "individually"?, "outside_only"?, "tool_layers_only"? (objects on layers with output off), "continuous"?: bool (default true), "speed_mm_s"?, "pointer"?: "red" \| "focus" \| "both", "blanking"?, "live"?: bool, "max_seconds"?: 1..=3600 (default 120) }` → red-pointer framing (light only) until frame stop, stop, a run, or the time limit; `live` traces the new outline whenever the document changes (`machine` event `frame_updated`); a focus pointer needs the profile's `focus_light_port_bit` (else 400) |
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

**Job control, calibration and device settings** (`crates/engine/src/api_machine.rs`, `job.rs`):

| Method | Path | Body / result |
|---|---|---|
| GET | `/api/job` | `{ active, run_id, state: "idle" \| "starting" \| "waiting_for_start_input" \| "running" \| "paused", iteration, iterations (null = continuous), completed, packets_sent, packets_acknowledged, packets_total, part, parts, paused, elapsed_s (stands still while paused, never goes back), estimated_time_s, remaining_s, progress (0..1: the time estimate held between the packets acknowledged and sent, `job::blended_progress`; below 1 until the job ends), progress_source ("time" \| "packets"), count, stop_at, end_after_current }` for the active device; also `GET /api/machine` `job` |
| PUT | `/api/job/settings` | `{ "stop_at": n \| null }` → the job status (Stop At) |
| POST | `/api/job/count/reset` | Reset Count of the active device → the job status |
| POST | `/api/job/end_after_current` | a repeating or continuous job ends after its current iteration → `{ ok, active }` |
| POST | `/api/machine/pause` | → `{ ok, running }`; not gated |
| POST | `/api/machine/resume` | optional `{ origin }` → `{ ok }`; UI only (403 otherwise); no job 409 |
| POST | `/api/machine/pointer` | `{ x, y, pointer?, max_seconds? }`: the red dot held at a field point (red-dot jog; light only) |
| POST | `/api/machine/axis` | `{ "z_mm" }` or `{ "steps" }` (+ `origin`): move the extension axis; UI only; no axis 400; a Z lift's `steps` must stay within its travel (400); claims the machine while it moves (a run in progress 409) |
| GET | `/api/machine/info` | `{ connected, profile, laser_source, laser_kind, fiber_kind, axis, serial_words?, version_words?, serial_hex? }` (GetSerialNo / GetVersion, read again when idle; words not decoded) |
| GET | `/api/preview.png` | `?width=64..=8192` (default 1600), `?jumps=false`: the preview as a PNG download (Save Preview Image) |
| POST | `/api/calibration/pattern` | `{ size_mm, layer? }`: adds the lens test pattern (square, centre lines, diagonals; group "Lens calibration") to the document → `{ created, group, measure: [field names] }`; marking it is an ordinary run |
| POST | `/api/calibration/solve` | `{ size_mm, measurements: { top_mm, bottom_mm, middle_x_mm, left_mm, right_mm, middle_y_mm, diagonal_up_mm, diagonal_down_mm, top_shift_mm?, right_shift_mm? }, apply? }` → `{ current, calibration, applied }` for the active device (`lmc::solve_lens_calibration`: per axis scale, trapezoid and bulge from the three widths/heights; the diagonals give the sum of the X and Y skews, and the corner shifts measured against a straight edge along the machine axes (top-left against bottom-left in X, bottom-right against bottom-left in Y) tell them apart: with both shifts both skews come from them, with one the other is the diagonals' sum minus it, with none the sum goes to X skew; repeat to refine); `apply` is UI only and stores it in the device (the built-in device is read-only: 409) |
| PUT | `/api/devices/{id}` | `{ profile?: JSON merge patch of the profile, display_name? }` → `{ ok, device, reactivated, warnings }`; UI only; `machine::edit_profile`: unknown keys, invalid values, a correction file with folders, a raised hard limit (power, jump speed, delays, mark speed, frequency range, pulse table; against the profile the device was built on, else its own) and port bit changes are 400; the built-in device 409; the active device is set up again (409 while a run or framing holds the machine) |
| POST | `/api/devices/{id}/duplicate` | → **201** `{ ok, device }`: an editable copy (source `manual`); UI only |
| GET | `/api/devices/{id}/lbdev` | the device as a LightBurn `.lbdev` (`machine::to_lbdev`: typed keys only, the profile's values, galvo 2 as X) |
| PUT / DELETE | `/api/devices/{id}/cor` | `?name=<file>.cor` with the file as the body (≤ 4 MB; parsed first) stores it in `<config-dir>/devices/` and sets the profile's `cor_file`; DELETE clears it; UI only (`?origin=`); the active device re-uploads its table; 409 (nothing stored) for the active device while a job holds the machine or framing is on, or when a different file of the same name is used by another device |
| POST | `/api/devices/import/markcfg` | `?name=` with an EzCad `markcfg7` as the body → **201** `{ ok, device }` (source `markcfg`, `MachineProfile::from_markcfg`); UI only |
| GET | `/api/sim/inputs` | `{ simulated, bits }`: the simulated board's input port word (`bits` null on USB) |
| POST | `/api/sim/inputs` | `{ "bits": 0..=65535 }` → `{ ok, bits }`: sets the simulated input port (`ReadPort` word 1), so the foot pedal (Start Marking, also Require Full Pulse), Stop Marking, Door Protect and Start Framing inputs can be exercised end to end; **403** while the link is a real board, or with MCP client headers (a switch press stands for a person) |

`GET /api/preview` segments carry `t_s`, the estimated time from the job's start to the end of the segment (the plan's
time model including pauses and dots), for a time slider; a dot (dot mode) is a zero-length mark.

`RunOptions` also take `queue: [project names]` (Batch: the saved projects are read and planned one by one and
joined into one job with one approval, at most 50; not with `selection` or `from_segment`) and `table: {increment,
settle_ms?, return_to_start?}` (Repeat Marking with an external axis table: the profile's `axis` with role
`linear_table` or `rotary_table`, `steps_per_mm` steps per mm or per degree; after each iteration the table moves
to `increment x completed` and waits `settle_ms`, and returns to 0 when the job finishes; unverified on hardware).
`POST /api/runs/{id}/approve` takes `checklist: [bool]` (see "Safety model").

`Run` = `{ id, state, origin, revision, stale, summary, stats, packet_count, created_unix_s, error?, report?, options, parts, completed }`
(`options` the run's `RunOptions`, `parts` the lists it is split into, `completed` the iterations run to the end); `summary` reads like `12 polylines on "Fill", "Outline", 340.2 mm marked, about 8.1 s` (layer names, each once, in burn order).
`state` is `"pending"` → `"approved"` → (`"awaiting_confirm"` →) `"running"` → `"finished"` \| `"failed"` \|
`"aborted"` (stopped while streaming), or `"cancelled"` (stopped before streaming, stale, or physical input not
pressed in time). `stale` is true for a pending run whose document has changed since; it cannot be approved.

Events on `/api/events`:

- `{ "type": "document_changed", "revision": n, "origin": Origin, "document_id" }` (after an edit, undo or redo, or when
  another open document became the active one; `document_id` is the open document, see "Open documents"; a client
  that fell behind gets `revision: -1, origin: null` and should refetch everything)
- `{ "type": "documents_changed", "active": id }` (a document was opened, closed or made active; refetch
  `GET /api/documents`)
- `{ "type": "art_library_changed", "library": name | null }` (an Art Library changed, or with `null` the list of
  libraries; refetch `GET /api/art/libraries`)
- `{ "type": "machine", "event": "framing" | "frame_stopped" | "frame_updated" | "run_approved" | "awaiting_confirm" |
  "waiting_for_start_input" (also sent as the older "awaiting_start_input" until the web UI moves over) | "run_started" | "run_finished" | "paused" | "resumed" | "job_count" | "axis_moved" |
  "stopped" | "error" | "lagged" | "lightburn" | "job_finished_sound" | "prefs", "detail"?: "..." }` (refetch `GET /api/machine`; after `lightburn`
  also `GET /api/lightburn`; after `job_count` also `GET /api/job`)
- `{ "type": "job_progress", "status": <GET /api/job> }` every 500 ms while a job holds the machine
- `{ "type": "run_pending", "run_id", "origin", "summary" }`
- `{ "type": "run_finished", "id", "ok": bool, "state", "error": string | null }`
- `{ "type": "approval_locked", "until_unix_s": n }` (too many wrong PINs; approvals locked until then)
- `{ "type": "project_saved", "name", "revision", "document_id" }` (refetch `GET /api/project`)
- `{ "type": "device_changed", "id": "<device id>" }` (another device was made active; refetch `GET /api/devices` and
  `GET /api/machine`) and `{ "type": "devices_changed" }` (devices imported or removed; refetch `GET /api/devices`)
- `{ "type": "history_changed", "marker": n, "steps": n, "revision": n, "document_id" }` (after every edit, undo, redo,
  goto or step delete, right after `document_changed`; refetch `GET /api/history`)

**Open documents** (LightBurn's File > New Window, UX-017; `crates/engine/src/documents.rs`). The engine keeps one
live session per open document; one is **active**, and every route and MCP tool that names no document (execute,
undo, history, project save/open, uploads, previews, runs, framing, autosave) uses it, so single-document clients
work unchanged on the first document, `default`. Each document has its own timeline and project state.
`GET /api/documents` → `{active, documents: [{id, name, project, display_name, document_name, revision, dirty,
objects, layers, active}]}`; `POST /api/documents {project?, activate? (default true)}` → **201** `{ok, id, active,
existing, result?}` (a new document laid out for the active device, or project `project` opened into one; a project
already open activates its document instead, **200** `existing: true`; at most 16); `POST
/api/documents/{id}/activate`; `POST /api/documents/{id}/close {discard?}` (unsaved changes 409 without `discard`;
the last document 409; closing the active one activates the one opened before it). Activation raises the activated
document's revision above every revision any document has had (`Session::raise_revision`), so revisions stay unique:
another document's pending run is stale, the preview cache never mixes documents and Require Framing Before Start asks
again; a clean or saved document stays clean. It emits `documents_changed`, then `document_changed` and
`history_changed` for the new active document. MCP: `documents_list` and `document_activate {id}`. A run records
the document it was made from (`runs[].document_id`); approval refuses one whose document is not the active one,
besides the revision check. A project opened into a new document is loaded while that document is still inactive and
only then activated, so a switch another client makes meanwhile cannot have it replace a different document. Only the
active document is autosaved.

**Art Library** (LightBurn's Art Library window, DES-074 / UX-045; `crates/engine/src/artlib.rs`, whose module docs
list the routes under `/api/art/libraries`). Libraries live in `<projects-dir>/art-libraries/<name>.oeart`, so every
client of the engine shares them: New, Load (an `.oeart`), Download, Rename, Delete; items: Add Selection to Library
(`{ids, name?}` from the active document), Import (a file, as `POST /api/document/import` formats, one item each),
Rename, Delete, copy or move to another library, thumbnails (128 px PNG rendered when added) and insert into the
active document (`art.insert {content, name?, center?, layer?}`: new ids, groups kept, objects on the layer of the
same colour or a new one of that name and colour, several pieces wrapped in one group, centred on `center`; one undo
step). Items read from a file or passed to `art.insert` must pass a project file's checks
(`doc::file::check_document`: unique ids, sizes, names, image data, groups not inside themselves); a stored thumbnail
that is not a PNG is rendered again; parsed libraries are kept (checked against the file's size and modification
time) so a window's thumbnails do not re-read the file each. An item is a `commands::ArtFragment` (objects, their groups, their layers' names and colours; no cut settings,
as in LightBurn). LightBurn's `.lbart` is a binary format with no public description and no sample was available, so
it is not read (415 with the reason).

**LightBurn settings that will not be executed** (LAY-034, `lbrn::unexecuted_settings`): per layer, the kept
`lbrn2.*` tags whose LightBurn switch is on but OpenEtch's own option is off (Wobble, Perforation, Tabs, Flood Fill,
Auto-rotate, Kerf Offset, Dot Mode) and unknown tags (`active: null`); gantry-only and bookkeeping tags are left out.
`doc.import_lbrn2` warns for the active ones and returns `unsupported_settings`; `GET /api/preview` adds
`unsupported` to each layer that has some; `GET /api/layers/unsupported` lists them for the active document.

**Libraries, defaults, logs, diagnostics, camera** (`crates/engine/src/{api_libs,libs,library,prefs,joblog,
diagnostics,updates,camera}.rs`; the module docs of `api_libs` list every route):

- **Material library** (LightBurn's Library window): `.clb` files in `<config-dir>/libraries/` (`lbrn::Library`:
  every real library is written back byte for byte, also by the structural writer), the loaded ones and the
  selected one in `libraries/index.json`; `GET /api/library` gives the tree material → thickness or title →
  description with each entry's `LinkPath` and a settings summary. Load (the body stored byte for byte), New, Select,
  Unload (unsaved changes need `discard`), Save / Save As, Rename, Merge; entries: Create new from layer, Update (from
  a layer), Edit Cut (complete CutSettings, checked like `layer.apply_preset`), Edit Description (material,
  thickness or title, description), Duplicate, Delete, Download. Entry edits stay in memory until saved. Applying:
  `POST /api/library/{name}/entries/{id}/apply {layer, link?}` runs `layer.apply_preset` (Assign copies; Link sets
  `Layer.link {library, path}` and `lbrn2.LinkPath`, and `layer.update` then refuses settings changes until
  `layer.unlink_preset`). Linked layers follow their preset: an edited entry updates them, an edited description
  relinks them to the new path, a deleted entry unlinks them, selecting or loading a library relinks every layer
  whose path it has (LightBurn), a renamed library carries its links along (each as one `script` undo step). A
  library saved, loaded or created while a device is active is that device's library and is selected with it.
- **Defaults** (per device, `<config-dir>/preferences.json`): `POST /api/layers/{id}/default` saves the layer's
  settings and sub-layers for its colour (`{all: true}`: Make Default for All, one default for every colour);
  `layer.add` without settings starts from them (the engine passes `defaults` into the recorded invocation); `POST
  /api/layers/{id}/reset` applies the saved default (`builtin`: OpenEtch's); `load_defaults_on_new` gives `doc.new`
  (and the first document at start) the default of the new layer's colour (`layer_settings`). Optimization: `POST
  /api/optimization/default` (Set as Defaults; new documents start with them through `doc.new`'s `optimization`),
  `DELETE` forgets them, `POST /api/optimization/reset {builtin?}`.
- **Job log**: one JSON line per ended run (time, run, state, origin, device, project, summary, duration,
  iterations, error) appended to `<config-dir>/job-log.jsonl`; `GET /api/job-log?limit=&format=csv`.
- **Play Sound on Job Finished** (per device): a finished run sends `machine` event `job_finished_sound`; the UI
  plays it.
- **Diagnostics**: a tracing layer keeps the last 2000 log lines (info and up; debug of OpenEtch's crates while
  `debug_log` is on), masking `token=`, `Bearer` and `pin=` values; `GET /api/diagnostics/report` is the bug report
  bundle (versions, machine status without run summaries, job log tail, the document's counts, the log lines; no
  document content, token or PIN).
- **Update check** (opt-in `check_for_updates`): `POST /api/updates/check` asks GitHub's releases API for
  `krzim/OpenEtch` through the system `curl` (no TLS stack in the engine) and compares versions.
- **Find My Laser / Add Galvo**: `GET /api/devices/scan` lists BJJCZ USB boards (`machine::detect`, builds with
  `usb`); `POST /api/devices/wizard {name?, markcfg_base64, cor_name?, cor_base64?, activate?}` (UI only) makes the
  device from EzCad's `markcfg7`, stores its correction file (the device is taken back if that fails) and can
  activate it.
- **Camera** (unverified on hardware): frames from a browser (`POST /api/camera/frame`) or, with feature `camera`
  (`nokhwa`, Apache-2.0), a UVC camera on the engine's computer (`POST /api/camera/capture`); a checkerboard detector
  (`POST /api/camera/detect`); lens calibration (two radial terms about the image centre, fitted so each view's
  corners are a perspective image of a flat grid); field alignment from at least four image points with known field
  positions (homography); `GET /api/camera/overlay.png?px_per_mm=` resamples the frame into the field square, +Y
  up. Calibrations per device in `<config-dir>/camera.json`. **Markers** (`markers.rs`, pure Rust): `generate.camera_markers
  {positions?, size_mm = 8, settings?}` adds square ArUco-layout fiducials (6 x 6 cells, OpenEtch's own 16-code
  dictionary in `commands::camera_markers`, every two codes 5+ cells apart in every turn; OpenCV's dictionaries are
  not included, so markers printed by other tools are not recognised) on a fill layer, each object keeping its id in
  `extras` `camera.marker`; `POST /api/camera/markers` finds them in the frame (adaptive threshold, both polarities,
  convex-hull quads, cells read through a homography, one cell error tolerated) and `POST /api/camera/align_markers`
  (UI only) aligns with the centres of those found that the document has (at least four).

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
and "Clean whole image" applies to 3D Sliced images (`settings.image.clean_whole_image`, see "Files and image
tools").
Commands (indices are 0-based positions in `sub_layers`; validation and limits as `layer.update`, at most 64
sub-layers; each one undo step): `layer.sub_add {layer, settings?, name?, cleanup?, index?}` (a copy of the main
settings with the partial `settings` merged in, like LightBurn's add; returns `index`), `layer.sub_update {layer,
index, settings?, name?, cleanup?}` (partial merge; `null` clears the name or the cleanup), `layer.sub_delete
{layer, index}`, `layer.sub_move {layer, from, to}`, `layer.sub_duplicate {layer, index}` (the copy goes right after
it; returns its `index`). Kept data of other software per sub-layer (`extras["<format>.subN.*"]`, N 1-based) moves
with its sub-layer on add, delete, move and duplicate. The web UI's "Add cleanup pass" uses LightBurn-like defaults:
the same mode and interval, one fast low-energy pass with the hatch turned 45°, cleaning after the main passes.

**Cut order and cut options** (`crates/plan/src/{job,order,cutopts,rotary}.rs`; `doc::job`). `Document.optimization`
(LightBurn's Optimization Settings, `job.set_optimization`, saved with the project): `enabled` (off outputs shapes in
drawn order), `order_by` [`layer` | `group` | `priority`] (layers always burn in list order; inside a pass shapes are
sorted by group rank, ungrouped objects together, then by `Document.object_options[id].priority`, 0 first),
`inner_first` (a closed shape inside another goes first), `direction_order` (top_to_bottom, ...), `reduce_travel`
(nearest first), `reduce_direction_changes` (OpenEtch's heuristic: a turn costs up to half the jump plus 0.5 mm),
`best_start` / `corners` (closed shapes start at the nearest point, or the nearest corner of 30° or more),
`best_direction`, `remove_overlaps` with `overlap_distance_mm` (line segments within the distance of an earlier one
are cut once; a trimmed loop becomes open lines) and `cuts_last` (line-mode layers after fill and image layers).
`Layer.global_passes` repeats the whole layer schedule (hatch angles keep turning). Mode `offset_fill` (LightBurn's
Offset Fill, `crates/plan/src/offsets.rs`): each fill group's closed outlines (even-odd) and their inward offsets
every line interval until nothing is left (round joins, so concave corners stay true), as closed loops ordered like
outlines; scan angle, cross-hatch and bidirectional do not apply. `options.kerf_offset_mm` (Kerf Offset, -50..=50):
line passes trace closed outlines offset outward (+) or inward (-), holes the opposite way (miter joins); open paths
are not offset. `ObjectOptions.start` (`object.set_start_point {id, node, reverse?}` / `object.clear_start_point`,
LightBurn's Set Start Point): the closed outline through that node starts at its nearest vertex (reversed when
asked) when "best start point" is off or optimization is off; as in LightBurn, best start overrides it. Each `CutSettings.options`
(`doc::CutOptions`, `layer.update` `settings.options`, partial): `output` (a sub-layer's own output toggle), Pause
Before / After (DelayTime words), `dot_mode` {dot_time_ms, dot_spacing_mm} (line passes become dots: JumpTo +
LaserOnPoint), `wobble` {step_mm, size_mm} (a circle per step, centred on the path), `fill_dot_width_mm` (both ends of
every hatch line shortened by half), `fill_grouping` (`individually`, the default kept for older files; `all_at_once`,
LightBurn's default, where overlaps cancel even-odd; `by_group`), `auto_rotate` (the hatch turns by the increment per
run of the job, the job counter, instead of per pass; repeats of one approval keep one angle), `flood_fill` (the hatch
split into connected regions), `perforation` {cut_mm, skip_mm}, `tabs` {count | spacing_mm, size_mm} (gaps on
closed outlines), `z_offset_mm` / `z_step_mm` (needs a board-driven Z axis, else a plan warning; each Z position is a
part). Line options apply in that order after ordering: tabs, perforation, wobble, dots. `hatch.lines_per_inch` sets
the interval to 25.4 / LPI. Per-object `power_scale_pct` (`object.set_cut_properties`) scales a shape's power,
images included. A source without pulse width (`pulse_width_enabled` false or a Q-switched `fiber_kind`) gets none,
with a plan warning.

**Rotary** (`Document.rotary`, `job.set_rotary`, saved with the project): chuck or roller, steps per rotation, object
and roller diameters, split width, cylinder correction, taper {end_diameter_mm, length_mm}. With `enabled`, runs split
the job into strips along the design's Y (the unwrapped surface; the work's axis lies along field X), map each strip
to the top of the work (`y = r(x)·sin(arc / R)` with cylinder correction, the radius following the taper), and move
the rotary between strips (`plan::JobPlan::parts`, one LMC list per part, a final move back to step 0). Previews show
the unwrapped design with a warning; a run needs a profile `axis` with role `rotary` (else 422), and a design more than one
turn of the work around (π · object diameter) is refused (it would mark over itself). Unverified on
hardware: the `MoveAxisTo` position word and direction follow meerk40t.

**Layer order** (LightBurn's Cuts / Layers order). Jobs burn layer by layer in `Document.layers` order
(`plan::plan_job`), so the list order is the cut order. `layer.move {id, to_index}` moves one layer to a 0-based
position (the others keep their order; one undo step; a no-op or an index past the end is refused) and returns the new
`order` of layer ids. The web UI reorders by drag and up/down in Cuts / Layers and by drag or Move up / Move down in
the Browser tree.

**Image engraving** (LightBurn's Image mode, `crates/plan/src/image.rs`). As in LightBurn, a layer's images always
engrave with its `settings.image` (each sub-layer has its own), while its vectors keep `mode`; power, speed,
frequency, pulse width, timing and passes are shared, and a pass burns its images first, then its vectors.
`ImageSettings {dither, interval_mm, angle_deg, angle_increment_deg, bidirectional, negative, pass_through,
dot_width_mm, min_power_pct, halftone_cells_per_inch, halftone_angle_deg}`; dither is `threshold`, `ordered` (8x8
Bayer), `atkinson`, `floyd_steinberg` (LightBurn's "Dither", `dithered` in its files), `stucki`, `jarvis`
(default), `newsprint` (45° clustered dots, 4 dots a cell), `halftone` (round dots at the given cells per inch and
angle), `sketch` (Sobel edges) or `grayscale`. Planning decodes the image once per job (`image` crate: PNG, JPEG,
GIF, BMP, WebP; transparent pixels never fire; shrunk on decode when the finest interval spans 4 or more pixels),
applies the image's `adjust` (enhance as an unsharp mask of three box blurs with the denoise threshold, then
contrast, brightness, gamma), then per pass samples square dots of the interval in rows along the scan angle (the
lattice is anchored at the field origin; each dot samples its centre through the inverse object transform, so
rotation and mirroring follow the object; pass-through uses the image's own pixel grid instead), inverts for
`negative`, dithers row by row from the lowest row (error diffusion keeps three rows), and merges consecutive "on"
dots into one two-point mark per run, clipped to the image outline. Dot width correction pulls each run end that
borders an unmarked dot in by half the width (never below a quarter of a dot); bidirectional alternates the rows
that mark. Grayscale runs merge equal levels (256) and carry their power, `min + (layer power - min) × level / 255`
(pure white never fires), in `PassPlan.powers`; the encoder sends a `MarkCurrent` before a run whose power
differs from the one in force (the list command the pass header uses; that it applies at that point of the list
on the B6 is unverified), and `machine::check_job` checks every `MarkCurrent` against the cap. Runs are cached per
image, settings and scan angle, so repeated passes cost no resampling. LightBurn's 3D Sliced mode (`3dslice`,
`slice3d`) engraves a depth map one grey-level slice per pass (see "Files and image tools").
Halftone cells per inch are LightBurn's `cellsPerInch` (LAY-033; seen next to `ditherMode halftone` in ComMarker's LightBurn `.clb` libraries), read and written like `halftoneAngle`. `layer.update` takes
`settings.image` partially (`dpi` sets `interval_mm` to 25.4 / dpi; dot width within 0..=interval, minimum power
0..=100); the plan refuses a grayscale minimum power above the machine limit.

**Ids and arranging.** Every command that takes object `ids` also takes group ids, which stand for all the group's
members (nested groups included). `object.duplicate` and `art.array` copy a fully selected group as a new group;
`object.delete` removes groups left empty. `object.align {ids, reference?, x?, y?, mode?}` follows LightBurn: the
default reference is the **last** id (the UI passes the selection in the order objects were added to it), which stays
put; other references are `first`, `selection`, `object {id}`, `stock` and `field`; a group aligns as one object by
its bounds; top is +Y. "Centre on stock" is `object.align` with `reference: {kind: "stock"}`, `x`/`y` `center`,
`mode: "group"`. `object.distribute {ids, axis, by: centers|gaps|spacing, spacing_mm?}` keeps the outermost objects
for centres and gaps. `object.move {ids, from_point, to_point}` is a snapped drag. `stock.move {center? | translate?,
rotation_deg? | rotate_by_deg?}` moves the stock only.

**Geometry tools** (`crates/geom`, commands in `crates/commands/src/builtin/{boolean,offset,path_tools,node,generate,
arrays,order}.rs`; LightBurn behaviour from its 2.x docs). All work in field mm and write results back into each
object's own frame (its transform is kept); every one is one undo step, returns `created`/`affected` (and `deleted`)
ids with bounds, and refuses locked objects and images. Booleans, weld, cut shapes and offsets run on outlines
flattened at 0.01 mm, so their results are polylines (an offset circle is a many-sided polygon); the other path tools
and node edits keep lines and Béziers. Editing a rectangle or ellipse turns it into a path, and editing an object's
geometry in place drops its `extras` (kept LightBurn shape data no longer describes it).
- `path.boolean {ids, op: union|subtract|intersect|xor}`: closed shapes as filled areas (each object even-odd, as jobs
  fill it; a group counts as one shape). Subtract cuts the **last** id out of each other object and deletes it
  (LightBurn: "the second shape you add ... is subtracted from the first, and deleted"); the others put the result in
  the first object. `path.weld {ids}`: non-zero fill per object, then union, one outline per island (LightBurn: weld
  can swallow contained shapes; counters drawn the other way round stay holes). `path.cut_shapes {ids}`: the last id
  (closed, ungrouped) cuts the others and is deleted; pieces on fill layers are closed, on line layers open; they go
  into new groups "Cut inside" / "Cut outside".
- `path.offset {ids, distance_mm, join: round|miter|bevel, miter_limit?, keep_original?, combine?, direction:
  outward|inward|both}`: open paths offset outward into a band with round ends; inward skips them.
- `path.break_apart {ids, mode: auto|segments|subpaths|islands}`: `auto` follows LightBurn's docs (several subpaths
  split into subpaths, a single one into its segments: a rectangle becomes four lines); `islands` keeps holes with
  their outer contour. `path.auto_join {ids, tolerance_mm = 0.05}` (LightBurn's fixed 0.05 mm; chains whose ends meet
  close), `path.close {ids, tolerance_mm?, mode: line|move_ends}`, `path.delete_duplicates {ids?, segments?}` (same
  layer and outline to 1 µm, whole document by default), `path.convert`, `path.optimize` (below), `path.fillet {ids,
  radius_mm, corners: "all" | [node]}` (negative radius bites; a whole rectangle keeps its corner radius), `path.trim
  {id, at, pick_mm?}`, `path.reverse`.
- `path.optimize {ids, tolerance_mm?, arcs_mm?, smooth_mm?}` follows LightBurn's Optimize Selected Shapes: each option
  has its own tolerance and they run smooth, arcs, lines (`geom::fit`: smoothing is Schneider's least-squares cubic
  fitting with tangent continuity, real corners over ~60° between longer lines kept; arc fitting greedily grows
  circles through runs of 4+ lines, every point and chord sagitta within the tolerance). Only runs of lines are
  refitted (existing curves stay). `path.fillet` rounds any corner (a node turning > 1°): line-line exactly as before;
  line-curve and curve-curve numerically (`geom::fillet`: the arc's centre is where both sides offset by the radius
  cross, refined by Newton steps; curve sides are cut with `subsegment`, so they stay one Bézier). `node.extend {id,
  at, pick_mm?, max_mm?}` (LightBurn's E) extends the open end nearest `at` straight on to the nearest line of any
  vector object or the shape itself (`geom::extend`).
- `path.warp {ids, mode: perspective|envelope|grid, corners? | grid?, tolerance_mm?}` (Warp Selection / Deform
  Selection, `geom::warp`): the selection's bounds map through a homography (corners must stay convex; lines stay
  lines), a bilinear envelope, or a bicubic patch of 4 x 4 handles (evenly spaced handles are the identity); each
  segment is split until its mapped control points draw it within the tolerance; splitting may add at most
  `geom::warp::MAX_WARP_SEGMENTS` segments over the whole selection.
- `object.transform` takes `keep_copy` (transform copies and leave the originals; LightBurn's Mirror Across Line copy
  option, for any transform). `object.move_to_page_position {ids, anchor, margin_mm?, reference: workspace|stock}` is
  the Move Selected Objects window. `art.array` takes `spacing_x`/`spacing_y` (`center` | `gap`: padding between
  edges), `reverse_x`/`reverse_y`, `row_shift`/`col_shift` (mm or `"half"`) and `mirror_alternate_cols`/`rows`
  (`x`|`y`, about each copy's centre).
- Generators: `art.tangent_circle_candidates {ids}` (Query) and `art.tangent_circle {ids, pick? | near?}` (LightBurn's
  Generate Tangent Circle: the circles tangent to three true circles, the Apollonius problem, up to 8, in
  `geom::construct`); `art.rubber_band {ids}` (Create Rubber-Band Outline: the convex hull of the outlines flattened
  at 0.01 mm); `art.toolpath_to_path {ids?, layer?}` (one path per layer and sub-layer with its planned marks, first
  repeat only, planned with a permissive target so field and power limits do not apply).
- Selection queries (Query, LightBurn's Edit menu): `select.open_shapes {ids?}`, `select.open_on_fill {ids?}` (open
  shapes on layers with a Fill, Fill+Line or Offset Fill pass), `select.contained {ids}` (the selection plus every
  shape whose flattened vertices and edge midpoints all lie inside a selected closed outline, even-odd). They return
  `ids`.
- Shapes: `Geometry::Polygon {sides, radius_mm, star_ratio?, corner_radius_mm}` is a parametric polygon or star
  (outline from `geom::shapes`, the same as `shape.polygon`'s path; first corner up); `shape.polygon {parametric:
  true}` makes one (the default stays a path until the web UI draws the `polygon` kind). LightBurn's `.lbrn2` has no
  polygon shape (none in the owner's 26 sample files), so exports write it as a path and DXF/SVG/PDF use its outline.
  `shape.update {id, ...}` is Shape Properties: rectangle `width_mm`/`height_mm`/`corner_radius_mm` (a kept radius
  shrinks to fit), ellipse `rx_mm`/`ry_mm`, polygon `sides`/`radius_mm`/`star_ratio` (null clears)/`corner_radius_mm`;
  centre, rotation and layer stay; fields that do not apply are refused.
- `art.nest {mode: "shapes", resolution_mm?}` nests true shapes (`geom::nest::shapes`): outlines and the region
  (field, rect, the real stock outline, or an object's closed outline) are rasterised on one grid of at most 512 cells
  a side; each part's cells cover it grown by half a cell diagonal and placed parts block cells within spacing plus
  that, so spacing and edge gap hold however coarse the grid. Largest part first, each at its lowest (then leftmost)
  free place over its rotations, searching only offsets that keep the part on the grid (a part larger than the region's
  bounds is skipped); bit-row tests are bounded (6e8 word compares), parts left are `unplaced` with `budget_exhausted`.
  Spacing and edge gap are capped at the region's diagonal (larger changes nothing) and discs are stamped as row spans,
  so a huge gap costs no more than one that blocks everything. Outlines are flattened at 0.02 mm and that is added to the gaps.
- Guides: `Document.guides` (`{id, axis: x|y, position_mm}`, ids from the document counter, left out of JSON while
  empty) are saved with the project (a loaded file's guide ids count for `next_id` and must be unique among all ids,
  at most `doc::MAX_GUIDES`, within ±`doc::MAX_GUIDE_MM`); `guide.add {axis, position_mm}`, `guide.move {id, position_mm, axis?}`,
  `guide.delete {ids?}` (all when left out). They never plan, export or burn.
- Output curve tolerance: `Document.curve_tolerance_mm` (`doc.set_curve_tolerance {tolerance_mm | null}`) wins over the
  device's `MachineProfile.curve_tolerance_mm` (`plan::Target.curve_tolerance_mm`), else 0.01 mm
  (`doc::DEFAULT_CURVE_TOLERANCE_MM`); both within 0.001..=1 mm. `plan` flattens vector objects at it
  (`Target::curve_tolerance`); display regions and image masks keep their own tolerances.
- `node.list {id}` (Query) and `node.move/insert/delete/break/join/set_type/segment_type`: nodes and segments numbered
  across subpaths (see `geom::nodes`); points in field mm, paths stay local under the transform.
- `shape.polygon`, `shape.line`, `shape.circle` (an ellipse with equal radii); `art.circular_array {ids, count, center,
  angle_deg = 360, rotate_copies}`, `art.along_path {ids, path_id, count | spacing_mm, rotate_to_path}` (count includes
  the original), `art.nest {ids, region: stock|field|{min,max}|{object}, spacing_mm, edge_mm, rotations, mode?}`
  (default `boxes`: Quick Nest-style bounding-box shelves; boxes never interlock; a round stock uses its inscribed
  square; `shapes`: true-shape nesting, below).
- `object.order {ids, to: front|back|forward|backward}`: the object list order (front = end, drawn on top). Jobs burn
  layer by layer and order each layer's shapes by nearest-neighbour travel (`plan::order`), so this only breaks ties
  there and does not set the cut order. `object.lock {ids, locked}`: `Document.locked` (ids; on the document so
  `Object` literals elsewhere stay valid); locked objects still select, copy, group, rename and burn, but transform,
  move, align, distribute, place, set_layer, delete and every path/node edit refuse them (`image.adjust` does not check
  yet). `object.make_same {ids, dimension, keep_aspect?}` (reference: the last id, or the only locked one),
  `object.rotate_to` / `object.scale_to {ids, pivot, from, to}` (two-point), and `object.transform` takes `shear_deg`
  and `mirror_line {a, b}`.


**Text** (LightBurn's Text tool, Variable Text and Create Bar Code; `crates/text`, commands in
`crates/commands/src/builtin/text.rs`). `Geometry::Text(doc::Text)` holds the settings (`text`, `font {family, kind:
system|stroke|shx|jsf}`, `height_mm`, `h_spacing` / `v_spacing` %, `align_h` left|middle|right, `align_v`
top|middle|bottom, `bold`, `italic`, `upper`, `welded`, `bend_deg` + `distort`, `path {object, offset_mm}`,
`max_width_mm` + `squeeze`, `ignore_empty_vars`, `variable {mode: date_time|serial|csv|cut_setting, offset}`) and the
rendered `outline` (LightBurn's `BackupPath`), which `local_path` returns: planning, previews, fill regions and exports
read only the outline and never need fonts. Every text command renders again (`text` crate; a missing font fails the
command). Layout, matched to LightBurn's saved outlines (`crates/text/tests/lightburn.rs` reproduces all 26 Text shapes
of the owner's files within 0.02 mm): `H` is the em size; each line is aligned by its ink (LightBurn 2.x; 1.x centred
by the advance); lines are `ascent + descent + line gap` apart; Align Y middle puts the first baseline at
`((n - 1) · pitch + line) / 2 - line`, top puts the capitals of line 1 on the origin, bottom the last baseline; HSpace
adds `LS` % of the height between characters. Max width scales lines to fit (squeeze: horizontally only). Bend wraps
the widest line over `bend_deg` of a circle (centre below for positive), glyphs rigid or warped (`distort`). Welded
merges overlapping glyphs only (polygons within 0.01 mm), the rest keep their curves. Fonts: installed families via
fontdb (lazily scanned), `.ttf/.otf/.ttc/.shx/.jsf` from the folders in `OPENETCH_FONT_DIRS` (`;` on Windows, `:`
elsewhere), and 12 bundled Hershey fonts (`kind: stroke`, never filled or welded); bold/italic use the family's own
faces (a missing italic is slanted, a missing bold is reported). `.jsf` (EzCad) and `.shx` (AutoCAD) layouts are
described in `jsf.rs` / `shx.rs`; the owner's 162 EzCad fonts all parse; no real `.shx` with a licence that allows
committing it was found (ixmilia/shx's `ISO3098B.SHX` comes from a page with no licence, tatarize/shxparser marks its
test fonts "of dubious providence", ezdxf ships none), so SHX is tested on synthetic files, plus any real fonts in
`OPENETCH_SHX_SAMPLES` (`text::shx` `real_shx_fonts_parse`, read in place); Asian big fonts and GB2312 JSF codes are not mapped. LightBurn's own single-line font format is
undocumented and not read.
- Commands: `shape.text {text, font?, height_mm? (5), ...the settings, center, rotation_deg, layer, name}`,
  `text.update {id, ...partial settings}` (`variable: {mode: normal}` turns variable text off; an update that changes
  nothing is refused, so it leaves no empty undo step; a name still equal to the auto name of the old text follows
  the new text; `shape.barcode {id, ...}` likewise refuses a no-op redraw and renames a "<kind> barcode" name when the
  kind changes),
  `text.apply_path {id, path, offset_mm?}` (the text takes the path object's transform; Align X from the path's start
  / centred / to its end, Align Y baseline / cap middle / cap top on the path), `text.detach_path {id}`,
  `text.convert_to_path {ids}` (also `path.convert`; permanent), `text.refresh {ids?}` (after a path moved, fonts were
  installed or a layer's settings changed), `font.list {kind?}` (Query).
- Variable text (LightBurn's Variable Text Formatting codes; single quotes copy text): serial `d`/`h`/`H` runs, `0`
  pads, a short run shows the last digits, two runs or mixed kinds are `bad serial format`; date/time Qt codes (`d dd
  ddd dddd M MM MMM MMMM yy yyyy h hh H HH m mm s ss z zzz AP ap t`, plus `w` / `W` ISO week; English names; `t` is the
  UTC offset); CSV `%N` (row = current + offset; `%%` a percent); cut settings `s S p P m M n d D i I L z Z q Q k K`
  and `C##` (the layer with that LightBurn index, else the ##th layer). `Document.variables` ({start, end, current,
  advance_by, auto_advance, csv: {name, rows}}, LightBurn's `<VariableText>`) is shared by every variable text:
  `var.set_serial {start?, end?, current?, advance_by?, auto_advance?, reset?}`, `var.advance {steps?, auto?}` (wraps
  between start and end), `var.load_csv {content, name?, reset_range?}`, `var.clear_csv`, `var.preview_row {value?}`
  (Query, LightBurn's Test). Each re-renders the variable texts. Run hooks: `POST /api/runs` plans from
  `commands::prepare_output(doc)` (variable texts rendered for now, so dates show the run's time; a text that cannot
  be rendered refuses the run; plain texts, also on paths, burn the outline the canvas shows, so a moved text or a
  deleted path never changes the burn), and `AppState::advance_variables_after_run` runs `var.advance {auto:
  true}` (one undo step, origin `script`) when `commands::auto_advance_due`; the run-completion code calls it after a
  run ends `finished`.
- Barcodes: `shape.barcode {kind, data, module_mm? (0.5), height_mm? (1D, 15), quiet_zone?, invert?,
  error_correction?, force_square?, show_text?, variable?, center, rotation_deg, layer, name, id?}`: qr, data_matrix, aztec,
  pdf417, code128, code39, code93, codabar, ean8, ean13, upc_a, upc_e, itf, telepen (the ZXing encoders; LightBurn's
  other 14 types, e.g. Micro QR, AprilTag, are not available). The modules are welded into a `Geometry::Path`; the
  parameters stay in `extras["openetch.barcode"]` so `id` redraws it and variable barcodes follow the counter.
  `show_text` writes the data under 1D codes (default font, a fifth of the bar height).
- LightBurn files: Text shapes import as `Geometry::Text` from `Font` (Qt font string: family, weight 50/75 or 400/700,
  style), `Str`, `H`, `LS`, `LnS`, `Ah` (0 left, 1 middle, 2 right), `Av` (1 middle; 0 top and 2 bottom assumed),
  `Weld`, with the `BackupPath` as the outline; all attributes stay in `extras` and are written back in their spelling
  while unchanged (byte-identical round trips), and an edited text writes its new values and outline. `VariableOffset`
  is kept (written from `variable.offset`); LightBurn's attribute names for upper case, bend, path, max width, ignore
  empty vars and the variable mode are unknown, so those are not written (a warning says so; the outline shows them).
  `<VariableText>` maps to `Document.variables` and is written back with its values.

**Projects.** Named projects are `<projects-dir>/<name>.openetch` (`--projects-dir`, default the OS data dir +
`openetch/projects`, e.g. `%APPDATA%\openetch\projects`, `~/Library/Application Support/openetch/projects`,
`~/.local/share/openetch/projects`; created on the first save). **Folders and locations** (UX-013; LightBurn saves
anywhere through OS dialogs, OpenEtch is a server): a project name may sit in up to 4 folders, `/`-separated
(`Clients/Acme/badge`, each part following the name rules; `templates` and `art-libraries` are never project
folders); further directories are mounted as locations under a label (`--projects-mount <label>=<path>`, repeatable,
and the list saved in `<config-dir>/project-locations.json` by `PUT /api/projects/locations {locations: [{label,
path}]}`, UI only, absolute existing directories), and a name whose first folder is a label lives there
(`NAS/coaster`). `GET /api/projects` lists every folder and location (`folder` on each), `GET /api/projects/folders`
gives `{folders, locations}`, `POST /api/projects/folders {name}` makes one; folders and files are both checked to stay
inside their root. Backups and autosaves of a project in folders use its flat name (`Clients__Acme__badge`). A name is 1-100 characters of letters, digits, space,
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
one without changing it), `--sim-pin <digits>` (simulator only: a test operator PIN held in memory, see "Safety
model"); `openetch devices list` and `openetch devices import <file> [--add all|<name>]` (both
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
are UI-only, and the server instructions say so; design-only devices refuse `frame_start` and `job_request_run`), and
`documents_list` / `document_activate {id}` (the open documents; every other tool works on the active one). The
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
  centred on the local origin, plus `adjust` (LightBurn's Adjust Image, left out while it changes nothing); jobs
  engrave images in Image mode (see "Image engraving").
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
  groups, Text becomes live text (`Geometry::Text`, see "Text") drawn and burned as its saved outline (BackupPath)
  until it is edited, exported as LightBurn text again, bitmaps become images (their `Data` rows are bottom-up in LightBurn, so
  the import adds a vertical flip to the transform and the export removes it; `CutSetting_Img` tags map to
  `settings.image` and the Bitmap's `Gamma`, `Contrast`, ... to the image's `adjust`, see `crates/lbrn/src/image.rs`);
  anything else (unknown shape types, the
  thumbnail, `UIPrefs`, `Notes`, root attributes, layout) is kept in `extras`. Export writes it all back, with the
  source text of every number, transform and path list that did not change, so an unchanged import exports byte for
  byte (the env-gated `crates/lbrn/tests/real_samples.rs` checks every sample file). `.lbt` templates read the same
  way. Export keeps each layer's LightBurn index and writes the stock outline on tool layer T1 (T2 when T1 is in
  use). **Tool layers** (LAY-042): `Layer.tool` (`T1` / `T2`) is LightBurn's tool layer: it never outputs (planning
  skips it whatever `output` says; `layer.update` refuses `output: true`), job framing includes its shapes (LightBurn
  frames tool layers with the job; `tool_layers_only` frames them alone), it exports as `CutSetting type="Tool"` at
  index 30 / 31 and the import sets it for those indices. `layer.tool {tool, id?}` adds one (named T1 / T2,
  LightBurn's colour) or makes a layer one (`tool: null` clears it). Commands:
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
  combined object), `image.adjust {ids, adjust: {gamma?, contrast?, brightness?, enhance_radius?, enhance_amount?,
  denoise?}, reset?}` (partial; images only) and `art.import_image {data_base64, mime, width_mm?, layer?, name?}` (PNG, JPEG, GIF,
  BMP, WebP; size from the file's resolution, else 254 dpi, fitted to 80 % of the field). Groups: `group.create {ids,
  name?}`, `group.ungroup {group}` (one level), `group.rename {group, name}`.
- **Presets** (`engine/src/test_presets.rs`, shared by every client, in the config directory): material test presets
  (LAY-070; LightBurn's Material Test Preset list) are named `generate.material_test` parameters,
  `/api/material-test/presets` (GET, PUT/DELETE `/{name}`, `POST .../import` and `GET .../export` of LightBurn's
  `material_test_presets.lbmt`, read from the owner's own file: JSON `name → {XParam/YParam (1 power, 2 interval,
  3 passes, 4 frequency kHz, 5 Q-pulse; 0 = speed inferred), XMin/XMax/XCount, XSize/YSize, XCenter/YCenter (from the
  field's lower-left), MaterialCut/TextCut/BorderCut as LightBurn cut settings}`, `commands::testgen_presets`; spacing,
  label height and title have no place in it). The owner's presets use 30 values a side, so a material test axis takes
  up to 30. Adjust Image presets (OTH-022) are `{name, adjust?, image?}` at `/api/image-adjust/presets` (GET, PUT/DELETE
  `/{name}`, `POST .../import` of the GET JSON). **Adjust Image unknowns**: LightBurn documents what Gamma, Contrast,
  Brightness and Enhance (unsharp mask: radius, amount, denoise "in smooth areas") do but no formula, range or unit;
  OpenEtch's curves (`plan::image::adjust`) follow the documented directions, its file scale (Contrast/Brightness as
  fractions) rests on the one sample with values (`photo on namecard.lbrn2`: Contrast 0.2, Brightness -0.11), and the
  file scale of `EnhanceRadius/Amount/Denoise` is unknown (every sample has 0); LightBurn's built-in "Basic" and "Black
  Paint on White" presets and its preset export file are undocumented, with no sample.
- **`.clb`** (LightBurn material library, `crates/lbrn/src/library.rs`): `lbrn::parse_clb` / `write_clb` round trip
  byte for byte (BOM, CRLF, two-space indentation, `" />"`, attribute spelling; unchanged entries from their source
  text, changed ones in the file's style). Material → Entry `Thickness` (`-1.0000` = none) / `Desc` / `NoThickTitle`
  → `CutSetting` (the project tags) with `LinkPath` `material/title/description`. `cut_setting_from_layer` and
  `LibEntry::to_layer` convert. `lbrn::parse_library` (flat entries) stays for the older callers. LightBurn's
  `Offset Fill` cut type maps to mode `offset_fill`.

## Files and image tools

Crate `fileio` (pure and wasm-ok except PDF reading), commands in `crates/commands/src/builtin/{files,image_tools,
pages}.rs`, routes in `crates/engine/src/userfiles.rs` (its module docs list them). LightBurn behaviour from its docs
(File Management, Trace Image, Convert to Bitmap, Apply Mask to Image, Image Options, Save Processed Bitmap, Print,
Pages, Show Notes, Managing Preferences, User Bundles, Import 3D Model, 3D Sliced Engravings).

- **Exports** (`doc.export {format: svg|dxf|ai|pdf, ids?, colors: layers|black, stroke_mm?, images?, tolerance_mm?,
  keep_circles?, area: bounds|workspace}`, Query; `GET /api/export` downloads it named after the project). The
  selection (`ids`, groups expanded) or everything, as LightBurn. SVG: mm, one Inkscape layer group per layer in its
  colour, fill layers filled even-odd, output-off layers written hidden (as LightBurn), text as outlines, images
  embedded with their transform. DXF: R12 ASCII (`AC1009`, `$INSUNITS 4`), one DXF layer per layer with the nearest
  AutoCAD colour index (hidden layers off), POLYLINE per subpath (curves flattened to `tolerance_mm`), CIRCLE for
  circles unless `keep_circles: false`; no images (warning). AI: an Illustrator 9+ file is PDF-compatible, so the AI
  export is the PDF. PDF: one page of the area, paths in the layer colours (filled for fill layers), images as RGB +
  alpha soft mask. `doc.print {format: pdf|svg, colors: black (default)|layers, line_width_mm (0.25), paper_mm?,
  margin_mm (10), area: workspace (default)|bounds}` (`GET /api/print`, inline): LightBurn's Print (black only / keep
  colors); with a paper size the design is centred and scaled down to fit (a warning gives the scale); hidden and
  output-off layers are left out. `image.export {id}` (the file as imported) and `image.processed {id, sub_layer?}`
  (Save Processed Bitmap: `plan::processed_bitmap` decodes, adjusts, applies negative and the layer's dither on the
  image's own grid at the line interval; PNG with its dpi; `GET /api/images/{id}/file`, `.../processed.png`).
- **Imports** (each Edit, one undo step; placement as `art.import_dxf`: `layer`, `name`, `width_mm`,
  `keep_position` (default centred), `split` (default one object per path in a group named after the file)).
  `art.import_pdf {data_base64, page?, hidden_layers?}` (native only; lopdf): the page's paths, rectangles and form
  XObjects with the CTM, in mm; optional content that is off (hidden Illustrator layers) unless `hidden_layers`;
  image XObjects as image objects where the page draws them (JPEG kept as is; 8-bit gray/RGB/CMYK/ICC/indexed and
  1-bit gray decoded to PNG with the soft mask as alpha; stencil masks, inline images, JPEG 2000/CCITT/JBIG2 left out
  with a warning); text and shadings left out with a warning. Page resources given by reference are followed. `art.import_ai`: PDF-based AI through the PDF reader (an AI without PDF
  content is refused with LightBurn's advice, "Create PDF Compatible File"), older PostScript AI through the EPS reader.
  `art.import_eps`: no permissive PostScript interpreter exists in Rust (Ghostscript is AGPL), so `fileio::postscript`
  interprets the subset vector art uses (path, arc, rect, transform, gsave/grestore, colour operators, `def`/`bind`/
  `load`/`exec`, `if`/`ifelse`/`repeat`/`for`, stack and arithmetic operators; Illustrator's one-letter operators when
  the file is Illustrator's, with its procsets skipped; DOS EPS preview headers); fonts, text, images and `loop` are
  not run (warning). LightBurn's current import list no longer has EPS. `art.import_hpgl {content, units_per_mm = 40}`:
  `IN DF SP PU PD PA PR CI AA AR EA ER RA RR` (one path set per pen); `LB`, `SC`, `IP`, `PE` skipped with a warning.
  `art.import_gerber {content, kind: auto|gerber|excellon, invert?}` (LightBurn 2.2): RS-274X copper as closed
  outlines (`FS`, `MO`, apertures C/R/O/P with holes, `LP` clear polarity cut out, G01/G02/G03 in G74/G75, D01/D02/D03,
  regions G36/G37; draws stroked with round ends through `geom` offsets, everything unioned), `invert` gives the board
  area outside the copper; Excellon drill files as circles of the tool diameter. Aperture macros and step-and-repeat
  are skipped with a warning. `art.import_model {data_base64, format?, rotation_deg?, width_mm?, dpi = 254,
  sixteen_bit?, invert?, transparent_background?, depth_cutoff_mm?, equalize?, outline?}` (LightBurn 2.2's Import 3D
  Model): STL (binary/ASCII), OBJ, OFF, PLY (ASCII and binary), 3MF (with the production extension's part
  references and item transforms) and AMF, rotated X then Y then Z, seen from above and z-buffered into a grey depth
  map image (top white, floor black: black engraves deepest, LightBurn's convention; background black or transparent;
  `depth_cutoff_mm` below the top is the floor, else the lowest point); `outline` also adds the silhouette as a path
  (Generate Outline); the image keeps the options (`extras` `model3d.options`) and the model zipped
  (`model3d.source`, up to 16 MB zipped) so `image.edit_model {id, ...options}` (LightBurn's Edit 3D Model) renders it
  again in place, redrawing its outline. `art.import_ezd {data_base64, pen_layers = true}` (not in LightBurn): EzCad 2
  `.ezd` objects (layout from meerk40t's MIT reader, checked on the owner's files: header offsets, pens, the
  Huffman-coded vector stream, curves, rectangles, circles, ellipses, polygons, groups, combines and hatch outlines;
  the generated hatch lines are dropped so a fill layer hatches the outlines again). Each pen becomes a line layer
  (speed, power, frequency, loops, pulse width), each hatch a fill layer (first enabled hatch's spacing and angle,
  further hatches as sub-layers, marked contours as fill + line; the hatch type flags are not decoded). Text: its
  glyph outlines when the file keeps them (the owner's file does), else live text in the named font when installed,
  else a placeholder rectangle and a warning. Images: the embedded bitmap as PNG (meerk40t's layout; no sample has
  one, unverified). `art.import_image` also
  takes TIFF and TGA, converted to PNG (a TIFF's resolution kept) so browsers show them and plans decode them. Legacy
  `.lbrn` projects (`<V vx vy c0x c0y c1x c1y/>` / `<P T p0 p1/>` path elements instead of `VertList`/`PrimList`; from
  LightBurn's forum, no sample file) are rewritten to `.lbrn2` text by `fileio::lbrn_legacy` inside
  `doc.import_lbrn2`, so `.lbrn` uploads and imports work like `.lbrn2`. `POST /api/document/import?format=...&mode=
  merge|replace` takes every format; `mode=replace` is LightBurn's Open of a vector or image file: a new, unsaved
  document (`doc.new` then the import, one step) named after the file. `tool_layers=true` is LightBurn's "Allow
  Importing to Tool Layers" (UX-034, default off): `layer` may then be `T1` / `T2` (added in the same step when the
  document has none; `util::layer_or_default` resolves those names); off, those names are refused and an import
  without `layer` skips a tool layer listed first. A `layer` id is used as given (the web UI applies the preference
  to the active layer itself).
- **Trace Image** (`image.trace`, `image.trace_preview` (Query, the dialog's live preview as SVG path data)):
  `fileio::trace`, written for OpenEtch (no potrace, which is GPL): pixels in `cutoff..=threshold` (0..128 default),
  or `sketch` (darker than the local mean) or `transparency` (opaque), limited to a `boundary` field rectangle;
  areas and holes under `ignore_less_than` pixels dropped; boundaries between traced and untraced pixels joined into
  loops (diagonal pixels connect), reduced to a polygon within 0.5 + `optimize` px (0.2 default), corners sharper than
  `smoothness` (0 lines only .. 1.33 curves only, default 1) kept, the others Bézier curves through the edge
  midpoints. `centerline` (LightBurn 2.2): Zhang-Suen thinning, the skeleton walked into open polylines between ends
  and junctions, spurs shorter than `ignore_less_than` pruned. The result is one path object on the image's layer
  (`layer`, `name`), following the image's transform; `delete_image`.
- **Convert to Bitmap** (`art.convert_to_bitmap {ids, dpi = 254, render: cut_settings|fill_all|outlines, gray = 128,
  line_width_mm?, keep_original?}`): an anti-aliased grey PNG of the vectors' bounds (drawn pixels at LightBurn's 50 %
  grey by default, white never fires); the vectors are deleted unless `keep_original`, as LightBurn.
- **Apply Mask to Image** (`image.apply_mask {image, mask}`, `image.remove_mask {ids}`, `image.flatten_mask {ids,
  crop?}`): non-destructive as in LightBurn: `Document.image_masks` links the image to a closed vector object or a
  group on one layer (even-odd, so an inner shape cuts a hole); the mask stays editable (put it on a tool layer);
  planning clips the image's scan runs to the mask (`plan::image::clip_runs`); deleting the mask or `remove_mask` shows
  the whole image again; `flatten_mask` bakes it into the PNG's alpha (and `crop` cuts the image to the masked
  pixels, LightBurn's Crop Image). Display of the mask in the canvas: UI pending.
- **Image Options** (`image.replace {id, data_base64, mime?, fit: file_size|fit|stretch}`): Replace Image (the file's
  own size), Replace Image to Fit (inside the old size) or stretch, keeping place, rotation, layer, adjustments and
  mask. Refresh Image is a replace with the edited file: the engine never reads the user's disk, so the UI sends the
  file again.
- **3D Sliced** (`settings.image.dither: slice3d`, LightBurn's `3dslice`; `plan::job::slice_level`): main pass k of N
  fires where the darkness is above (N - k) / N, so the first pass engraves only the deepest (darkest) areas and every
  pass a larger area, and a dot of darkness d gets ceil(d · N) passes (pure white none), matching LightBurn's preview
  ("black marks the engraved area of each layer, travel expands with each pass"). A cleanup sub-layer cleans the
  slice of the main pass just before it, or everything with `settings.image.clean_whole_image` (LightBurn's "Clean
  whole image"; no LightBurn tag for it was seen, so it is not exported). Depth on the B6 is unverified on hardware.
- **Projects**: `Document.notes {text, show_on_load}` (`doc.set_notes`; LightBurn's `Notes ShowOnLoad Notes` element
  is read into it and written from it; showing them on open is the UI's job). Saving over a project first copies the
  old file to `<projects-dir>/.backups/<name>.<unix>.openetch` (newest `backups_keep`, default 5); autosave writes a
  changed document every `autosave_minutes` (default 5, 0 off) to `<projects-dir>/.autosave/<name or
  Untitled>.<unix>.openetch`, newest `autosave_keep` (10) per name (`/api/backups`, `/api/backups/restore` opens one
  as an unsaved document; settings in `<config-dir>/files.json`, `/api/files/settings`). Thumbnails:
  `GET /api/projects/{name}/thumbnail.png` renders the saved project (`render.rs`). Templates: `<projects-dir>/
  templates/` holds `.openetch` templates (Save As Template, `/api/templates/save`) and LightBurn `.lbt` files
  (`/api/templates/upload`); `/api/templates/open` opens one as a new untitled document, so a template is never
  overwritten.
- **Pages** (LightBurn 2.2's Pages window; `page.list`, `page.add`, `page.duplicate`, `page.delete`, `page.rename`,
  `page.activate`, `page.move`, `page.send_objects`): `Document.pages` in order and `active_page`; the active page's
  shapes are the document's own `objects`/`groups` (so previews, jobs and every command work on it) and the others'
  live in their `Page` entries with their locks, cut properties and masks; layers and settings are shared, as in
  LightBurn. How LightBurn stores pages in `.lbrn2` is unknown (only `CurrentPage`/`Page0Name` root attributes were
  seen), so LightBurn exports carry the active page.
- **Preferences and bundles**: `GET /api/prefs/export` writes the engine's preferences (`prefs.rs`) and the file
  settings as an `.oeprefs` JSON file; `POST /api/prefs/import` (UI only) replaces them after backing the current ones
  up in `<config-dir>/prefs-backups/` (newest 50, also made at start when they changed; `/api/prefs/backups`,
  `/api/prefs/backups/restore`). User bundles (`GET /api/bundle/export?parts=prefs,devices,libraries`, an
  `.oebundle` zip with a `bundle.json` manifest; `POST /api/bundle/import`, UI only): devices are always added as new
  devices with their correction files (an identical device is skipped, a different correction file of the same name
  is kept and reported), a material library whose name exists with other content loads as "<name> (bundle)", as
  LightBurn keeps what is there. LightBurn's own `.lbzip` and `.lbprefs` layouts are undocumented and not read.
  A bundle is a file from elsewhere: entry names must be plain file names (no folders, no drive prefix such as
  `C:x.cor`, which `Path::join` resolves outside the directory on Windows, no device names); an imported device must
  name its correction file by bare name and keep this machine's limits (`machine::edit_profile` against its base,
  else the built-in profile), or it is skipped; an unloaded library's file is never overwritten; preferences are
  checked whole (Job Checklist, file settings) before anything changes.
- **Hostile files**: every importer bounds its work, not only its input size. The zip reader refuses overlapping
  entries before unpacking (a few kilobytes pointing hundreds of entries at one block); the PostScript interpreter
  charges a step per procedure call (empty procedures in nested loops), caps the operand stack, `{` nesting, array
  nesting and path elements, shares strings and arrays instead of copying them, and computes `arc` turns with
  `rem_euclid`; the PDF reader loads with lopdf's decompression limit and decodes at most 32 MB per content stream
  and 256 MB in total (forms that draw themselves); Gerber/Excellon and EzCad refuse non-finite or huge coordinates
  and sizes before curve flattening (a 1e300 circle at 0.001 mm never finishes); the model readers trust no header
  count, cap 3MF component expansion and refuse depth maps needing over 2e9 pixel tests up front; placed art must
  lie within 100 m of the origin; thinning follows the shrinking boundary and Douglas-Peucker is iterative and
  chunked (a long diagonal edge recursed past the stack). Inactive pages are validated like the document on load,
  and `doc.export_lbrn2` warns when other pages are left out. Flatten Image Mask deletes the mask shapes, as
  LightBurn does.
- Not done: several open documents (LightBurn's New Window; the engine holds one live document), saving to arbitrary
  folders (the engine writes only inside its projects directory; the browser downloads anywhere).

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
  the field to 0.1 mm: the B6's 153.8 is EzCad's `FIELDSIZE=153.75`; a `Height` more than 0.05 mm from it is a
  non-square field, `field_height_mm`, and exports write it back), `swap_xy`,
  `negate_x/y`, `cor_file` (file name), laser on/off/polygon/end delays, jump speed and delays, jump distance limit,
  `mark_speed.max_mm_s` (`MaxSpeed`, 7000 on the B6: only lowers it), frequency range (only narrowed),
  `pulse_width_enabled` (`Laser_QPulseWidthEnabled`), `open_mo_delay_us` (`Laser_OpenMODelay`, default 8000),
  `start_mark_input_bit` (`StartMarkPortBit`, 14 on the B6, also read from markcfg `STARTMARKPORT`),
  `red_light_speed_mm_s` (`FrameSpeed`), `calibration` (`FieldOffsetX/Y`, `FieldAngle`, `Galvo_n_Scale/Skew/
  Trapezoid/Bulge`, galvo 2 as X unless swapped; see "Coordinate frames"). Every job list waits `open_mo_delay_us`
  after the MO opens; pulse widths are not sent when `pulse_width_enabled` is false; runs that ask for the start input
  wait for `start_mark_input_bit`. Hard limits are never raised; every change is validated one at a time, and clamps,
  refusals (a correction outside the model's range) and settings OpenEtch does not model (a red-light bit other than
  8, `CorScaleOverride`, ...) are reported as diffs. The result always passes `MachineProfile::validate`. Further
  profile fields set in OpenEtch only: `stop_mark_input_bit`, `focus_light_port_bit`, `blanking`, `laser_kind`,
  `fiber_kind`, `red_dot`, `axis`.
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
- **Editing** (`PUT /api/devices/{id}`, see "Engine API"): the profile of an imported, copied or EzCad-imported device
  can be changed with a JSON merge patch; hard limits can only go down; the built-in device is read-only (duplicate it).
  A re-import of the same LightBurn device replaces the edits. Correction files can be loaded and cleared per device;
  `.lbdev` export writes the device back for LightBurn.

## CAD integrations

`tools/fusion360-addin/OpenEtch` (Autodesk Fusion add-in, Python): "Send to OpenEtch" saves the selected sketch (or a
selected planar face's outline, or the sketch being edited) as DXF and posts it to `POST /api/document/upload
?format=dxf&mode=merge&name=...` with the engine's bearer token when one is set. `tools/inkscape-extension`
(`openetch_send.inx` + `.py`, Inkscape 1.x): "Save selection to OpenEtch" posts the selection (with its groups'
transforms and the document's size, so it keeps its place) as SVG. Neither can fire the laser. Their request and
selection code is tested without Fusion or Inkscape (`python -I tools/tests/test_integrations.py`).

## Licensing and provenance

Code ported from MIT/Apache projects keeps its notice in the file header and is listed in `ATTRIBUTION.md`. Never copy
from GPL/SSPL/unlicensed sources (Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader).
