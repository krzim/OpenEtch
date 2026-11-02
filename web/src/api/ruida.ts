// A Ruida gantry (RDC644x / 634x CO2 controllers over Ethernet; unverified on hardware): the New Ruida device form,
// the controller's File List window (LightBurn's File List: list, start, delete, and Send) and File > Save RD file,
// on the engine's POST /api/devices/ruida, GET /api/machine/files, POST /api/machine/files/send,
// /api/machine/files/{index}/run and /delete, and GET /api/export/rd. Starting a stored file fires the laser, so it
// only creates a pending run: it is approved in the Laser window's Runs card like any other run (the operator PIN or
// Start), never from here. Pure, so `npm test` covers it.

import type { DeviceOrigin } from "./devices.ts";
import type { MachineStatus } from "./types.ts";

/** The active machine is a Ruida gantry (GET /api/machine `driver`): the File List window and Save RD file are offered
 * only then. */
export function isRuidaMachine(m: MachineStatus | null | undefined): boolean {
  return m?.driver === "ruida";
}

/** A file stored on the controller: its 1-based index in the controller's list and its name. */
export interface StoredFile {
  index: number;
  name: string;
}

/** GET /api/machine/files `files`, read defensively (entries without a usable index or name are dropped). */
export function parseFiles(raw: unknown): StoredFile[] {
  const files = raw && typeof raw === "object" ? (raw as { files?: unknown }).files : null;
  if (!Array.isArray(files)) return [];
  return files.flatMap((f) => {
    const o = f && typeof f === "object" ? (f as Record<string, unknown>) : null;
    const index = o?.index;
    const name = o?.name;
    return typeof index === "number" && Number.isInteger(index) && index > 0 && typeof name === "string" ? [{ index, name }] : [];
  });
}

/** Longest name the engine accepts for a stored file (`machine::ruida::MAX_FILE_NAME`). */
export const MAX_FILE_NAME = 32;

/** Why `name` cannot name a file on the controller (1-32 printable ASCII characters), or null. */
export function fileNameProblem(name: string): string | null {
  if (name.length === 0) return "Give the file a name.";
  if (name.length > MAX_FILE_NAME) return `At most ${MAX_FILE_NAME} characters.`;
  if (!/^[\x20-\x7e]+$/.test(name)) return "Only plain ASCII letters, digits, spaces and punctuation.";
  return null;
}

/** A controller file name from a project name: ASCII only, at most 32 characters, "JOB" when nothing is left. */
export function suggestFileName(project: string | null | undefined): string {
  const base = (project ?? "")
    .replace(/\.[^.]*$/, "")
    .normalize("NFKD")
    .replace(/[^\x20-\x7e]/g, "")
    .trim()
    .slice(0, MAX_FILE_NAME)
    .trim();
  return base || "JOB";
}

/** Save RD file's name: the engine's, else the project's, ending in `.rd`. */
export function rdFileName(engineName: string | null, project: string | null): string {
  if (engineName && /\.rd$/i.test(engineName)) return engineName;
  const base = (project ?? "").replace(/\.[^.]*$/, "").trim();
  return `${base || "job"}.rd`;
}

// ---- New Ruida device ------------------------------------------------------------------------------------------------

export interface RuidaForm {
  displayName: string;
  /** The controller's IP address. */
  address: string;
  widthMm: string;
  heightMm: string;
  origin: Exclude<DeviceOrigin, "center">;
  maxPowerPct: string;
  maxSpeedMmS: string;
  /** The RDC634XG scrambles with key 0x11 instead of 0x88. */
  rdc634xg: boolean;
}

export const RUIDA_FORM_DEFAULT: RuidaForm = {
  displayName: "Ruida laser",
  address: "",
  widthMm: "600",
  heightMm: "400",
  origin: "top_right",
  maxPowerPct: "100",
  maxSpeedMmS: "500",
  rdc634xg: false,
};

const IPV4 = /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;

/**
 * The body of POST /api/devices/ruida from the form, or the first problem as a plain sentence. The engine validates
 * the profile again (and is the one that matters); this only catches typing slips before the request.
 */
export function ruidaDeviceBody(f: RuidaForm): { body: { display_name: string; profile: Record<string, unknown> } } | { error: string } {
  const num = (v: string) => (v.trim() === "" ? NaN : Number(v));
  const name = f.displayName.trim();
  if (!name) return { error: "Give the device a name." };
  const address = f.address.trim();
  if (address && !IPV4.test(address) && !address.includes(":")) return { error: "The controller address must be an IP address, e.g. 192.168.1.100." };
  const w = num(f.widthMm);
  const h = num(f.heightMm);
  if (!(w > 0 && h > 0 && w <= 5000 && h <= 5000)) return { error: "The work area must be positive, at most 5000 mm each way." };
  const power = num(f.maxPowerPct);
  if (!(power > 0 && power <= 100)) return { error: "The power limit must be within 0–100 %." };
  const speed = num(f.maxSpeedMmS);
  if (!(speed > 0 && speed <= 5000)) return { error: "The speed limit must be positive, at most 5000 mm/s." };
  return {
    body: {
      display_name: name,
      profile: {
        address,
        magic: f.rdc634xg ? 0x11 : 0x88,
        work_area_mm: [w, h],
        origin: f.origin,
        max_power_pct: power,
        max_speed_mm_s: speed,
      },
    },
  };
}
