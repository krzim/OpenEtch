// GET /api/preview.bin (crates/engine/src/preview_bin.rs): the toolpath preview in a compact binary form, decoded into
// the same `Preview` shape GET /api/preview answers. A large job (an image is one segment per dot run) is tens of MB
// of JSON that a Raspberry Pi takes seconds to write and Wi-Fi seconds more to move; this is about a tenth of that
// before compression. Engines without the route answer 404 and the caller falls back to the JSON.
//
// Format (version 1, little-endian): "OEPV", u32 version, u32 header length H, H bytes of JSON header, then the
// sections the header's `sections` gives as [byte offset, element count]:
//   flags (u8 per segment): bit 0 mark (else jump), bit 1 emits, bit 2 own power (next `power` value), bit 3 starts
//     where the previous segment ended (its start is not stored);
//   xy (f32): per segment its start x, y unless bit 3, then its end x, y;
//   t (f32 per segment): t_s;  power (f32): power_pct of the segments with bit 2.
// `runs` in the header gives each stretch of segments' pass and layer.

import type { Preview, PreviewSegment, Vec2 } from "./types.ts";

const MARK = 1;
const EMITS = 2;
const POWER = 4;
const CHAINED = 8;

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
  sections?: Record<string, [number, number]>;
  stats?: Record<string, unknown>;
  passes?: Preview["passes"];
  layers?: Preview["layers"];
  warnings?: string[];
}

/** Decode a GET /api/preview.bin body. Throws on anything that is not a version 1 preview. */
export function decodePreview(buf: ArrayBuffer): BinaryPreview {
  const bytes = new Uint8Array(buf);
  const view = new DataView(buf);
  if (bytes.length < 12 || String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]) !== "OEPV") throw new Error("not a binary preview");
  const version = view.getUint32(4, true);
  if (version !== 1) throw new Error(`binary preview version ${version} is not supported`);
  const h = view.getUint32(8, true);
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(12, 12 + h))) as Header;
  const sec = (name: string): [number, number] => {
    const s = header.sections?.[name];
    if (!Array.isArray(s) || s.length !== 2) throw new Error(`binary preview without ${name}`);
    return s;
  };
  // DataView reads: no alignment or byte-order assumptions about the platform.
  const f32 = (name: string) => {
    const [at, n] = sec(name);
    if (at + n * 4 > bytes.length) throw new Error(`binary preview ${name} out of range`);
    return (i: number) => view.getFloat32(at + i * 4, true);
  };
  const xy = f32("xy");
  const t = f32("t");
  const power = f32("power");
  const [flagsAt, n] = sec("flags");
  if (flagsAt + n > bytes.length) throw new Error("binary preview flags out of range");
  const segments: PreviewSegment[] = new Array(n);
  let run = 0;
  let left = 0;
  let pass: number | undefined;
  let layer: string | null = null;
  let k = 0;
  let p = 0;
  let prev: Vec2 = [0, 0];
  const runs = header.runs ?? [];
  for (let i = 0; i < n; i++) {
    while (left === 0 && run < runs.length) {
      const r = runs[run++];
      left = r.count;
      pass = r.pass ?? undefined;
      layer = r.layer;
    }
    left--;
    const f = bytes[flagsAt + i];
    let from: Vec2;
    if (f & CHAINED) from = prev;
    else {
      from = [xy(k), xy(k + 1)];
      k += 2;
    }
    const to: Vec2 = [xy(k), xy(k + 1)];
    k += 2;
    prev = to;
    const s: PreviewSegment & { emits?: boolean; power_pct?: number } = { kind: f & MARK ? "mark" : "jump", layer, from, to, t_s: t(i), emits: (f & EMITS) !== 0 };
    if (pass !== undefined) s.pass = pass;
    if (f & POWER) s.power_pct = power(p++);
    segments[i] = s;
  }
  const preview: Preview = { segments, passes: header.passes ?? [], stats: header.stats ?? {}, layers: header.layers ?? [], warnings: header.warnings ?? [] };
  return { preview, revision: typeof header.revision === "number" ? header.revision : -1 };
}
