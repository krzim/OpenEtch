// A result photo from the iPad / phone camera or a file, scaled in the browser to at most 1280 px (JPEG) before it is
// sent, so a 12-megapixel camera shot travels as about 200 KB. The engine averages the middle half for the colour.

import { scaledSize } from "./model.ts";

/** The photo as `{base64, mime}` (JPEG, long side at most `max` px). */
export async function encodePhoto(file: Blob, max = 1280): Promise<{ base64: string; mime: string }> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch {
    throw new Error("the file is not an image this browser can read");
  }
  const { w, h } = scaledSize(bitmap.width, bitmap.height, max);
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no canvas");
  ctx.drawImage(bitmap, 0, 0, w, h);
  bitmap.close();
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.86));
  if (!blob) throw new Error("the photo could not be encoded");
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return { base64: btoa(bin), mime: "image/jpeg" };
}
