// HTTP calls for the routes in crates/engine/src/userfiles.rs (exports, print, image files, imports of every format,
// thumbnails, autosave and backups, templates, preference export/import and backups, user bundles). Every request
// carries the bearer token in the Authorization header, as api/client.ts does; downloads come back as blobs so they
// work with token auth. The UI never reads the engine's disk: files go up as request bodies.

import { ApiError, authToken, UI_ORIGIN } from "../api/client.ts";
import { contentDispositionName } from "../api/project.ts";
import { okError } from "../api/runs.ts";

function headers(accept: string, contentType?: string): Record<string, string> {
  const h: Record<string, string> = { Accept: accept };
  if (contentType) h["Content-Type"] = contentType;
  const t = authToken();
  if (t) h.Authorization = `Bearer ${t}`;
  return h;
}

async function send(method: string, path: string, init: { headers: Record<string, string>; body?: BodyInit }): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(path, { method, ...init });
  } catch (e) {
    throw new ApiError(`engine unreachable: ${e instanceof Error ? e.message : String(e)}`);
  }
  if (!res.ok) {
    const text = await res.text();
    let msg = text || res.statusText;
    let code: string | undefined;
    try {
      const o = JSON.parse(text) as { error?: unknown; code?: unknown };
      if (o && typeof o.error === "string") msg = o.error;
      if (o && typeof o.code === "string") code = o.code;
    } catch {
      // Not JSON: keep the text.
    }
    throw new ApiError(`${method} ${path.split("?")[0]}: ${res.status} ${msg}`, res.status, { code, detail: msg });
  }
  return res;
}

async function json<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await send(method, path, {
    headers: headers("application/json", body === undefined ? undefined : "application/json"),
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  const data = text ? (JSON.parse(text) as T) : (undefined as T);
  const err = okError(data, `${path} failed`);
  if (err !== null) throw new ApiError(err);
  return data;
}

async function upload<T>(path: string, body: Blob, contentType = "application/octet-stream"): Promise<T> {
  const res = await send("POST", path, { headers: headers("application/json", contentType), body });
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** A downloaded file and the name the engine gave it. */
export interface Download {
  blob: Blob;
  filename: string | null;
  /** `x-openetch-warnings`: how many warnings the export had. */
  warnings: number;
}

async function download(path: string): Promise<Download> {
  const res = await send("GET", path, { headers: headers("*/*") });
  return {
    blob: await res.blob(),
    filename: contentDispositionName(res.headers.get("Content-Disposition")),
    warnings: Number(res.headers.get("x-openetch-warnings") ?? 0) || 0,
  };
}

export const filesApi = {
  /** doc.export as a file (`query` from `exportQuery`). */
  exportFile: (query: string) => download(`/api/export?${query}`),
  /** doc.print, print-ready (`query` from `printQuery`). */
  printFile: (query: string) => download(`/api/print?${query}`),
  /** The image object's file as imported (LightBurn: Export of one selected image). */
  imageFile: (id: string) => download(`/api/images/${encodeURIComponent(id)}/file`),
  /** Save Processed Bitmap: the image as its layer (or sub-layer `sub`) will engrave it. */
  imageProcessed: (id: string, sub?: number) => download(`/api/images/${encodeURIComponent(id)}/processed.png${sub ? `?sub_layer=${sub}` : ""}`),
  /** POST /api/document/import with the raw file (`query` from `importQuery`): `{ok, result, revision}` or `{ok:false,error}`. */
  importFile: (query: string, file: Blob) => upload<unknown>(`/api/document/import?${query}`, file),
  /** The saved project rendered as PNG. */
  /** Project folders (UX-013): the tree and the mounted locations. */
  projectFolders: () => json<unknown>("GET", "/api/projects/folders"),
  createProjectFolder: (name: string) => json<unknown>("POST", "/api/projects/folders", { name, origin: UI_ORIGIN }),
  projectLocations: () => json<unknown>("GET", "/api/projects/locations"),
  /** UI only: replace the saved locations (the command line's stay). */
  setProjectLocations: (locations: { label: string; path: string }[]) => json<unknown>("PUT", "/api/projects/locations", { locations, origin: UI_ORIGIN }),
  thumbnail: (name: string, width = 128) => download(`/api/projects/${encodeURIComponent(name)}/thumbnail.png?width=${width}`),
  backups: (project?: string | null) => json<unknown>("GET", `/api/backups${project ? `?project=${encodeURIComponent(project)}` : ""}`),
  restoreBackup: (kind: "autosave" | "backup", file: string) => json<unknown>("POST", "/api/backups/restore", { kind, file, origin: UI_ORIGIN }),
  fileSettings: () => json<unknown>("GET", "/api/files/settings"),
  setFileSettings: (patch: Record<string, number>) => json<unknown>("PUT", "/api/files/settings", { ...patch, origin: UI_ORIGIN }),
  templates: () => json<unknown>("GET", "/api/templates"),
  saveTemplate: (name: string, overwrite: boolean) => json<unknown>("POST", "/api/templates/save", { name, overwrite, origin: UI_ORIGIN }),
  uploadTemplate: (name: string, file: Blob) => upload<unknown>(`/api/templates/upload?name=${encodeURIComponent(name)}`, file, "application/xml"),
  openTemplate: (name: string) => json<unknown>("POST", "/api/templates/open", { name, origin: UI_ORIGIN }),
  exportPrefs: () => download("/api/prefs/export"),
  /** A LightBurn .lbprefs / prefs.ini: `preview` maps it without applying (else UI only: applied, engine part only). */
  importLightBurnPrefs: (file: Blob, preview: boolean) =>
    upload<unknown>(`/api/prefs/import_lightburn?${preview ? "preview=1" : `origin=${UI_ORIGIN}`}`, file, "application/json"),
  importPrefs: (file: Blob) => upload<unknown>(`/api/prefs/import?origin=${UI_ORIGIN}`, file, "application/json"),
  prefsBackups: () => json<unknown>("GET", "/api/prefs/backups"),
  restorePrefs: (file: string) => json<unknown>("POST", "/api/prefs/backups/restore", { file, origin: UI_ORIGIN }),
  exportBundle: (parts: string[]) => download(`/api/bundle/export?parts=${encodeURIComponent(parts.join(","))}`),
  importBundle: (parts: string[], file: Blob) => upload<unknown>(`/api/bundle/import?parts=${encodeURIComponent(parts.join(","))}&origin=${UI_ORIGIN}`, file, "application/zip"),
};

/** Save a blob through a temporary download link. */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
