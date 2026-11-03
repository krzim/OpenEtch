// HTTP calls of the MOPA materials UI (crates/engine/src/api_materials.rs, its module docs list the routes). Same
// auth and errors as the libs calls (libs/api.ts). The hue step model and its calibration are UI only and carry
// `origin: "ui"`; nothing here frames, runs or approves.

import { UI_ORIGIN } from "../api/client.ts";
import { call, download } from "../libs/api.ts";

const enc = encodeURIComponent;
const entry = (lib: string, id: string) => `/api/library/${enc(lib)}/entries/${enc(id)}`;

export const materialsApi = {
  finish: (lib: string, id: string) => call("GET", `${entry(lib, id)}/finish`),
  /** Only the fields given change (`photo_base64` + `photo_mime` a new photo, `remove_photo`, `color: null` the photo's average). */
  finishEdit: (lib: string, id: string, patch: Record<string, unknown>) => call("PUT", `${entry(lib, id)}/finish`, patch),
  photo: (lib: string, id: string) => download(`${entry(lib, id)}/photo`),
  sidecarDownload: (lib: string) => download(`/api/library/${enc(lib)}/finish/download`),
  sidecarImport: (lib: string, file: Blob) => call("POST", `/api/library/${enc(lib)}/finish/import`, undefined, { body: file, type: "application/json" }),
  palette: (lib?: string) => call("GET", `/api/materials/palette${lib ? `?library=${enc(lib)}` : ""}`),
  hueModel: () => call("GET", "/api/materials/hue-model"),
  hueModelEdit: (model: Record<string, unknown> | null) => call("PUT", "/api/materials/hue-model", { model, origin: UI_ORIGIN }),
  calibrate: (best: string, other: string, hues: number) => call("POST", "/api/materials/hue-model/calibrate", { best, other, hues, origin: UI_ORIGIN }),
  nudge: (body: Record<string, unknown>) => call("POST", "/api/materials/nudge", { ...body, origin: UI_ORIGIN }),
  bracket: (body: Record<string, unknown>) => call("POST", "/api/materials/bracket", { ...body, origin: UI_ORIGIN }),
  pick: (layer: string) => call("POST", "/api/materials/bracket/pick", { layer, origin: UI_ORIGIN }),
  separate: (body: Record<string, unknown>) => call("POST", "/api/materials/separate", { ...body, origin: UI_ORIGIN }),
};
