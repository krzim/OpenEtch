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
`localStorage`, removed from the address bar, and sent as `Authorization: Bearer <token>` on every request and as
`?token=` on the WebSocket URL.

## How it works

- **No local document edits.** Every change is `POST /api/execute` (or `/api/batch`) with `origin: "ui"`. The UI
  refetches `/api/document` and `/api/history` on the WebSocket `document_changed` event and, as a fallback, after
  every execute response. Undo/redo use `POST /api/undo` and `POST /api/redo`.
- **Command ids and parameter shapes** live in one file, `src/api/commands.ts`, matching the input structs in
  `crates/commands/src/builtin`. At startup the UI compares them against `GET /api/commands` and shows
  "N cmd missing" in the top bar (hover for the list) if the engine lacks any. The **Commands** toolbar button runs any
  registered document command with raw JSON parameters: the same catalogue an agent sees over MCP.
- **Viewport** draws in field millimetres with +Y up (one SVG group carries the pan/zoom and the y-flip). The field
  square comes from `GET /api/machine` (`field_mm`), 153.75 mm until the engine reports it. Pointer Events throughout:
  drag empty space to pan, wheel or pinch to zoom, two fingers to pan, tap to select (shift adds), drag a selection to
  move it (one `object.transform` translate on release). Rectangle/Ellipse tools: drag to draw, tap for 20 mm.
- **Laser workspace**: toolpath overlay from `GET /api/preview` (marks coloured by layer, jumps dashed grey), machine
  status, and pending runs. Approving a run takes two taps ("Approve…" then "Confirm and fire") and the confirm step
  times out after 8 s. **STOP** is in the top bar in every workspace and is never gated.
- **Narrow screens** (< 1024 px, e.g. iPad portrait): the Browser and Properties panels become drawers toggled from the
  toolbar.

## Layout

```
src/
  api/types.ts        wire types (Document, HistoryEntry, Preview, MachineStatus...)
  api/client.ts       fetch wrapper, auth token, endpoints
  api/commands.ts     every command id + parameter builder the UI uses
  api/useEngine.ts    React hook: fetching, WebSocket events, refetch coalescing
  geometry.ts         affine maths, geometry -> SVG path, bounds (pure; tested)
  selection.ts        UI-only selection state (pure; tested)
  components/         TopBar, Toolbar, BrowserPanel, Viewport, PropertiesPanel, LaserPanel, Timeline, Dialogs, ui
```
