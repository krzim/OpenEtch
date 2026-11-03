// The pixel size of a WebP file, read from its header (lossy VP8, lossless VP8L and extended VP8X). guide-shots.mjs
// records each picture's size in docs/guide/img-sizes.json, so the guide can give every <img> its width and height
// (no layout shift while pictures load above an anchor); docs.test.ts checks the list against the files.

/** [width, height] of a WebP image, or null when `buf` is not one. */
export function webpSize(buf) {
  if (buf.length < 30 || buf.toString("ascii", 0, 4) !== "RIFF" || buf.toString("ascii", 8, 12) !== "WEBP") return null;
  const kind = buf.toString("ascii", 12, 16);
  if (kind === "VP8 ") return [buf.readUInt16LE(26) & 0x3fff, buf.readUInt16LE(28) & 0x3fff];
  if (kind === "VP8L") {
    const b = buf.readUInt32LE(21);
    return [(b & 0x3fff) + 1, ((b >> 14) & 0x3fff) + 1];
  }
  if (kind === "VP8X") return [buf.readUIntLE(24, 3) + 1, buf.readUIntLE(27, 3) + 1];
  return null;
}

/** docs/guide/img-sizes.json's text for the pictures in `dir`: { "name.webp": [width, height], ... }, one per line. */
export async function sizesJson(dir) {
  const { readdirSync, readFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const lines = readdirSync(dir)
    .filter((f) => f.endsWith(".webp"))
    .sort()
    .map((f) => {
      const s = webpSize(readFileSync(join(dir, f)));
      if (!s) throw new Error(`not a WebP file: ${f}`);
      return `  ${JSON.stringify(f)}: [${s[0]}, ${s[1]}]`;
    });
  return `{\n${lines.join(",\n")}\n}\n`;
}
