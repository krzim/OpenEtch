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
or replace the PIN hash file, drive the browser, talk to the engine like the UI does, or open the USB device itself.
Only the optional physical confirmation input and the hardware E-stop/key switch hold against that.

- Risk classes on every command: `Query`, `Edit`, `Pointer` (red light / motion, no emission), `Emit` (laser fires).
  `/api/execute`, `/api/batch` and MCP `execute`/`batch` accept only `Query` and `Edit`.
- Light framing uses a code path that **cannot** open the master oscillator or set the laser-enable port bit: it only
  emits jumps and the red-pointer port bit.
- An `Emit` action never runs from an API or MCP call directly. `POST /api/runs` / MCP `job_request_run` plan, encode
  and check the document into a *pending* run and send nothing. Only `POST /api/runs/{id}/approve` with the
  **operator PIN** turns it into an armed run (`approve` is the only engine module that creates `machine::Armed`, the
  token `Controller::run` needs). The endpoint refuses requests carrying MCP client headers or a non-`ui` origin, and
  a run whose document revision is stale.
- **Operator PIN.** Set once by a person with `openetch set-pin` (interactive; refused unless stdin is a terminal;
  4-12 digits; shown while typed, because std has no portable no-echo input). Stored as a salted SHA-256 hash in
  `<config-dir>/operator-pin` (`--config-dir`, default the OS config dir + `openetch`, e.g. `%APPDATA%\openetch`,
  `~/.config/openetch`) and re-read on every approval. Five wrong PINs in a row lock approvals for 60 s. MCP never
  receives or handles PINs, and the MCP instructions tell agents they cannot and must not approve runs or set PINs.
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
  above 5,000,000 segments, so one edit cannot make planning exhaust the engine's memory.
- The engine reaches the board's link only through `Controller`, except for two non-emitting uses with the controller
  locked: swapping in a freshly opened USB link before `init`, and `ReadPort` for the physical confirmation.

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
| GET | `/api/machine` | `connected`, `simulated`, `busy`, `state`, `running`, `framing`, `field_mm`, `profile`, `max_power_pct`, `last_error`, `sim_counters`, `approval: { pin_set, physical_confirm: null \| { input_bit, timeout_s } }`, `pending_runs: [Run]`, `runs: [Run]` |
| POST | `/api/runs` | optional `{ "origin" }` → **201** `{ "ok": true, "id", "run": Run, "message" }`; plans, encodes and checks the document; sends nothing to the board; at most 8 pending |
| POST | `/api/runs/{id}/approve` | `{ "pin": "<operator PIN>" }` → `{ "ok": true, "id", "state": "approved" \| "awaiting_confirm" }`; the run then starts in the background. PIN refusals are **403** `{ "ok": false, "error", "code": "pin_not_set" \| "pin_required" \| "pin_invalid" \| "pin_locked" }`; MCP headers or a non-`ui` origin 403; unknown run 404; not pending, stale revision or another run active 409 |
| POST | `/api/machine/frame` | `{ "ids"?: [...], "mode"?: "bounds" \| "outline" }` → red-pointer framing (light only) until frame stop, stop, a run, or 120 s |
| POST | `/api/machine/frame/stop` | ends framing only; pending and approved runs are untouched → `{ "ok": true }` |
| POST | `/api/machine/stop` | emergency stop: abort any run or frame, cancel pending runs and runs not yet streaming → `{ "ok": true, "connected": true, "cancelled_runs": n }`; with no board connected **200** `{ "ok": true, "connected": false, ... }`; no token needed |
| WS | `/api/events` | server → client JSON events (below) |
| * | `/mcp` | MCP Streamable HTTP endpoint |

`Run` = `{ id, state, origin, revision, stale, summary, stats, packet_count, created_unix_s, error?, report? }`.
`state` is `"pending"` → `"approved"` → (`"awaiting_confirm"` →) `"running"` → `"finished"` \| `"failed"` \|
`"aborted"`, or `"cancelled"` (stopped before streaming, stale, or physical input not pressed in time). `stale` is
true for a pending run whose document has changed since; it cannot be approved.

Events on `/api/events`:

- `{ "type": "document_changed", "revision": n, "origin": Origin }` (after an edit, undo or redo; a client that fell
  behind gets `revision: -1, origin: null` and should refetch everything)
- `{ "type": "machine", "event": "framing" | "frame_stopped" | "run_approved" | "awaiting_confirm" |
  "run_started" | "run_finished" | "stopped" | "error" | "lagged", "detail"?: "..." }` (refetch `GET /api/machine`)
- `{ "type": "run_pending", "run_id", "origin", "summary" }`
- `{ "type": "run_finished", "id", "ok": bool, "state", "error": string | null }`

**Access.** Every request's `Host` must be a loopback name, an IP literal or an `--allow-host` name, and an `Origin`
header, when present, must be loopback, allowed or same-host (DNS-rebinding and cross-site protection). On loopback
no token is needed. Off loopback (`--listen 0.0.0.0:7878` on the Pi) a bearer token is generated (or given with
`--token`) and printed in the startup URL: `/api/*` and `/mcp` need `Authorization: Bearer <token>`; only
`GET /api/events` (the WebSocket, where browsers cannot set headers) also accepts `?token=`. Static files and
`POST /api/machine/stop` need no token.

Default listen address `127.0.0.1:7878`. Other options: `--profile`, `--web-dir`, `--usb`, `--token`,
`--allow-host`, `--config-dir`, `--confirm-input`; `openetch set-pin [--config-dir <path>]` sets the operator PIN.

## MCP tools

`list_commands`, `execute`, `batch`, `inspect_document`, `render_preview` (PNG), `undo`, `redo`, plus separate machine
tools: `machine_status`, `frame_start`, `frame_stop`, `job_request_run`, `machine_stop`. The MCP surface is generated
from the same command registry the UI uses; machine tools are separate so clients can approve them separately. No
MCP tool can approve a run or touch the operator PIN, and the server instructions say agents cannot and must not.

## Licensing and provenance

Code ported from MIT/Apache projects keeps its notice in the file header and is listed in `ATTRIBUTION.md`. Never copy
from GPL/SSPL/unlicensed sources (Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader).
