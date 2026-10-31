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
  field mm to galvo units, never in `doc`.

## Safety model (enforced in code, not by convention)

- Risk classes on every command: `Query`, `Edit`, `Pointer` (red light / motion, no emission), `Emit` (laser fires).
- Light framing uses a code path that **cannot** open the master oscillator or set the laser-enable port bit: it only
  emits jumps and the red-pointer port bit.
- An `Emit` action never runs from an API or MCP call directly. `job.request_run` creates a pending run; only the
  engine's UI approval endpoint (or a configured physical input) turns it into an armed run. Stop/abort is always
  available and never gated.
- Hard limits (max power, field bounds) live in the machine profile and cannot be changed by command arguments.
- The hardware E-stop and key switch are the real safety system; the software is not.

## Engine API (HTTP, JSON)

| Method | Path | Body / result |
|---|---|---|
| GET | `/api/commands` | `[CommandSpec]` (id, title, description, risk, input_schema) |
| POST | `/api/execute` | `{ "id": "...", "params": {...}, "origin": "ui" }` → `{ "ok": true, "result": ..., "revision": n }` or `{ "ok": false, "error": "..." }` |
| POST | `/api/batch` | `{ "commands": [{id, params}], "origin": "ui" }` → one undo step; all-or-nothing |
| GET | `/api/document` | full `Document` JSON |
| GET | `/api/history` | `[HistoryEntry]` (revision, command id, origin, summary) |
| GET | `/api/preview` | simulated job: `{ segments: [{kind: "jump"|"mark", layer, from: [x,y], to: [x,y]}], stats }` in field mm |
| GET | `/api/preview.svg` | rendered preview (stock, objects, toolpath) |
| GET | `/api/machine` | machine status (connected?, sim?, busy, pending runs) |
| POST | `/api/runs/{id}/approve` | UI-only approval of a pending run |
| POST | `/api/machine/stop` | always allowed |
| WS | `/api/events` | server → client: `{ "type": "document_changed", "revision": n }`, `{ "type": "machine", ... }`, `{ "type": "run_pending", ... }` |
| * | `/mcp` | MCP Streamable HTTP endpoint |

Default listen address `127.0.0.1:7878`; `--listen 0.0.0.0:7878` on the Pi.

## MCP tools

`list_commands`, `execute`, `batch`, `inspect_document`, `render_preview` (PNG), `undo`, `redo`, plus separate machine
tools: `machine_status`, `frame_start`, `frame_stop`, `job_request_run`, `machine_stop`. The MCP surface is generated
from the same command registry the UI uses; machine tools are separate so clients can approve them separately.

## Licensing and provenance

Code ported from MIT/Apache projects keeps its notice in the file header and is listed in `ATTRIBUTION.md`. Never copy
from GPL/SSPL/unlicensed sources (Balor, balor-meerk40t, BurnCraft, LaserCraft, lmcv2u-loader).
