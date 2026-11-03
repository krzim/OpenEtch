// GET /api/preview.bin (crates/engine/src/preview_bin.rs): the toolpath preview in a compact binary form, decoded into
// the packed form the browser's preview worker gives (api/packedPreview.ts: typed arrays, image passes as rasters), so
// the page never builds one object per segment. A large job (an image is one segment per dot run) is tens of MB of
// JSON that a Raspberry Pi takes seconds to write and Wi-Fi seconds more to move; this is a fraction of that before
// compression, and an image costs its dots instead of its runs. Engines without the route answer 404 and the caller
// falls back to the JSON.
//
// Format (versions 1 and 2, little-endian): "OEPV", u32 version, u32 header length H, H bytes of JSON header, then
// the sections the header's `sections` gives as [byte offset, element count]:
//   flags (u8 per segment): bit 0 mark (else jump), bit 1 emits, bit 2 own power (next `power` value), bit 3 starts
//     where the previous segment ended (its start is not stored), bit 4 (version 2) drawn by a raster: no geometry;
//   xy (f32): per segment without bit 4 its start x, y unless bit 3, then its end x, y;
//   t (f32 per segment): t_s;  power (f32): power_pct of the segments with bit 2;
//   version 2: levels (u8), row_segments (u32), row_span (f32): the arrays of the header's `rasters`.
// `runs` in the header gives each stretch of segments' pass and layer.

import { EMITS as P_EMITS, MARK as P_MARK, LAYER_SHIFT, NO_PASS, rastersOf, type PackedSegments } from "./packedPreview.ts";
import type { Preview } from "./types.ts";

const MARK = 1;
const EMITS = 2;
const POWER = 4;
const CHAINED = 8;
const RASTER = 16;

/** A decoded preview and the document revision it is of. */
export interface BinaryPreview {
  preview: Preview;
  revision: number;
}

interface Header {
  version?: number;
  revision?: number;
  count?: number;
  runs?: { pass: number | null; layer: string | null; count: number }[];
  rasters?: unknown;
  sections?: Record<string, [number, number]>;
  stats?: Record<string, unknown>;
  passes?: Preview["passes"];
  layers?: Preview["layers"];
  warnings?: string[];
}

/** Decode a GET /api/preview.bin body. Throws on anything that is not a version 1 or 2 preview. */
export function decodePreview(buf: ArrayBuffer): BinaryPreview {
  const bytes = new Uint8Array(buf);
  const view = new DataView(buf);
  if (bytes.length < 12 || String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]) !== "OEPV") throw new Error("not a binary preview");
  const version = view.getUint32(4, true);
  if (version !== 1 && version !== 2) throw new Error(`binary preview version ${version} is not supported`);
  const h = view.getUint32(8, true);
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(12, 12 + h))) as Header;
  const sec = (name: string, optional = false): [number, number] => {
    const s = header.sections?.[name];
    if (!Array.isArray(s) || s.length !== 2) {
      if (optional) return [0, 0];
      throw new Error(`binary preview without ${name}`);
    }
    return s;
  };
  // Sections are 4-byte aligned from the start of the buffer: typed-array views where the platform is little-endian
  // (every browser), DataView reads otherwise.
  const little = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;
  const words = <T extends Float32Array | Uint32Array>(name: string, make: (b: ArrayBuffer, at: number, n: number) => T, read: (at: number) => number, optional = false): T => {
    const [at, n] = sec(name, optional);
    if (at + n * 4 > bytes.length || at % 4 !== 0) throw new Error(`binary preview ${name} out of range`);
    if (little) return make(buf, at, n);
    const out = make(new ArrayBuffer(n * 4), 0, n);
    for (let i = 0; i < n; i++) out[i] = read(at + i * 4);
    return out;
  };
  const f32 = (name: string, optional = false) => words(name, (b, at, n) => new Float32Array(b, at, n), (at) => view.getFloat32(at, true), optional);
  const xy = f32("xy");
  const t = f32("t");
  const power = f32("power");
  const [flagsAt, n] = sec("flags");
  if (flagsAt + n > bytes.length || t.length < n) throw new Error("binary preview flags out of range");
  const pk: PackedSegments = {
    xy: new Float32Array(4 * n),
    t: t.length === n ? t : t.subarray(0, n),
    info: new Uint32Array(n),
    pass: new Uint32Array(n),
    power: new Float32Array(n),
    layerIds: [],
  };
  const layerIndex = new Map<string, number>();
  let run = 0;
  let left = 0;
  let pass = NO_PASS;
  let layerBits = 0;
  let k = 0;
  let p = 0;
  let px = 0;
  let py = 0;
  const runs = header.runs ?? [];
  for (let i = 0; i < n; i++) {
    while (left === 0 && run < runs.length) {
      const r = runs[run++];
      left = r.count;
      pass = typeof r.pass === "number" ? r.pass : NO_PASS;
      if (r.layer) {
        let li = layerIndex.get(r.layer);
        if (li === undefined) {
          li = pk.layerIds.length;
          layerIndex.set(r.layer, li);
          pk.layerIds.push(r.layer);
        }
        layerBits = (li + 1) << LAYER_SHIFT;
      } else layerBits = 0;
    }
    left--;
    const f = bytes[flagsAt + i];
    pk.info[i] = ((f & MARK ? P_MARK : 0) | (f & EMITS ? P_EMITS : 0) | layerBits) >>> 0;
    pk.pass[i] = pass;
    const o = 4 * i;
    if (f & RASTER) {
      pk.xy[o] = pk.xy[o + 1] = pk.xy[o + 2] = pk.xy[o + 3] = NaN;
      pk.power[i] = NaN;
      continue;
    }
    if (!(f & CHAINED)) {
      px = xy[k++];
      py = xy[k++];
    }
    pk.xy[o] = px;
    pk.xy[o + 1] = py;
    px = xy[k++];
    py = xy[k++];
    pk.xy[o + 2] = px;
    pk.xy[o + 3] = py;
    pk.power[i] = f & POWER ? power[p++] : NaN;
  }
  if (k > xy.length || p > power.length) throw new Error("binary preview xy out of range");
  let rasters: Preview["rasters"] = [];
  if (version >= 2) {
    const [levelsAt, levelsN] = sec("levels", true);
    if (levelsAt + levelsN > bytes.length) throw new Error("binary preview levels out of range");
    const rowSegments = words("row_segments", (b, at, m) => new Uint32Array(b, at, m), (at) => view.getUint32(at, true), true);
    rasters = rastersOf(header.rasters, { levels: bytes.subarray(levelsAt, levelsAt + levelsN), rowSegments, rowSpan: f32("row_span", true) });
  }
  const preview: Preview = { segments: [], packed: pk, rasters, passes: header.passes ?? [], stats: header.stats ?? {}, layers: header.layers ?? [], warnings: header.warnings ?? [] };
  return { preview, revision: typeof header.revision === "number" ? header.revision : -1 };
}
