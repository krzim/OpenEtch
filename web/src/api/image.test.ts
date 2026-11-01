// Unit tests for api/image.ts: line interval <-> DPI, the image settings model and patch builder, and the preview
// pipeline (adjustments and every dither mode).

import assert from "node:assert/strict";
import { test } from "node:test";
import * as I from "./image.ts";
import type { CutSettings } from "./types.ts";

const flat = (v: number, w = 64, h = 64): I.GrayImage => ({ width: w, height: h, data: new Float32Array(w * h).fill(v) });
const opts: I.DitherOptions = { pixelMm: 0.05, halftoneCellsPerInch: 50, halftoneAngleDeg: 22.5, minPowerFraction: 0 };
const near = (a: number, b: number, eps: number) => assert.ok(Math.abs(a - b) <= eps, `${a} not within ${eps} of ${b}`);

test("DPI and line interval convert both ways", () => {
  near(I.dpiFromInterval(0.1)!, 254, 1e-9);
  assert.equal(I.intervalFromDpi(254), 0.1);
  assert.equal(I.intervalFromDpi(1016), 0.025);
  near(I.dpiFromInterval(0.05)!, 508, 1e-9);
  for (const dpi of [100, 300, 333, 508, 1000, 2540]) near(I.dpiFromInterval(I.intervalFromDpi(dpi)!)!, dpi, dpi * 1e-5);
  assert.equal(I.intervalFromDpi(300), 0.0846667);
  for (const bad of [0, -1, NaN, Infinity]) {
    assert.equal(I.dpiFromInterval(bad), null);
    assert.equal(I.intervalFromDpi(bad), null);
  }
});

test("image settings merge over the defaults and patch through layer.update", () => {
  const base = { mode: "fill", power_pct: 50 } as unknown as CutSettings;
  assert.deepEqual(I.imageSettingsOf(base), I.DEFAULT_IMAGE_SETTINGS);
  const s = I.imageSettingsOf({ ...base, image: { dither: "halftone" } } as unknown as CutSettings);
  assert.equal(s.dither, "halftone");
  assert.equal(s.interval_mm, I.DEFAULT_IMAGE_SETTINGS.interval_mm);
  assert.deepEqual(I.imageUpdate("L1", { dpi: 300 }).params, { id: "L1", settings: { image: { dpi: 300 } } });
  assert.deepEqual(I.imageUpdate("L1", { negative: true, interval_mm: 0.025 }), {
    id: "layer.update",
    params: { id: "L1", settings: { image: { negative: true, interval_mm: 0.025 } } },
  });
  // LightBurn's "Dither" is doc::DitherMode::FloydSteinberg.
  assert.ok(I.DITHER_MODES.some((m) => m.value === "floyd_steinberg" && m.label === "Dither"));
});

test("an image's adjust is absent while default, and is set with image.adjust", () => {
  assert.deepEqual(I.adjustOf({}), I.DEFAULT_ADJUST);
  assert.deepEqual(I.adjustOf(null), I.DEFAULT_ADJUST);
  const a = I.adjustOf({ adjust: { gamma: 1.5, contrast: 20 } });
  assert.deepEqual(a, { ...I.DEFAULT_ADJUST, gamma: 1.5, contrast: 20 });
  assert.deepEqual(I.changedAdjust(a), ["gamma", "contrast"]);
  assert.deepEqual(I.changedAdjust(I.DEFAULT_ADJUST), []);
  assert.deepEqual(I.imageAdjust(["o1", "o2"], { denoise: 4 }), { id: I.IMAGE_ADJUST_CMD, params: { ids: ["o1", "o2"], adjust: { denoise: 4 } } });
});

test("preview size follows the line interval and is capped", () => {
  assert.deepEqual(I.previewSize(10, 5, 0.1, 1000), { width: 100, height: 50, capped: false });
  assert.deepEqual(I.previewSize(100, 50, 0.01, 1000), { width: 1000, height: 500, capped: true });
  assert.deepEqual(I.previewSize(0.01, 0.01, 0.1, 1000), { width: 1, height: 1, capped: false });
});

test("grey from RGBA puts transparent pixels over white", () => {
  const g = I.grayFromRgba([0, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 255], 4, 1);
  assert.deepEqual([...g.data].map(Math.round), [0, 255, 255, 76]);
});

test("adjustments: defaults change nothing; contrast, brightness and gamma move the tones", () => {
  const ramp: I.GrayImage = { width: 5, height: 1, data: Float32Array.from([0, 64, 128, 192, 255]) };
  assert.deepEqual([...I.adjustImage(ramp, I.DEFAULT_ADJUST).data], [...ramp.data]);
  const bright = I.adjustImage(ramp, { ...I.DEFAULT_ADJUST, brightness: 20 }).data;
  assert.ok(bright[1] > 64 && bright[4] === 255);
  const contrast = I.adjustImage(ramp, { ...I.DEFAULT_ADJUST, contrast: 50 }).data;
  assert.ok(contrast[1] < 64 && contrast[3] > 192);
  const flat = I.adjustImage(ramp, { ...I.DEFAULT_ADJUST, contrast: -100 }).data;
  assert.ok(flat[0] > 110 && flat[4] < 145, "contrast -100 % leaves a 1/16 slope");
  // LightBurn's scales (the engine's plan::tone_factors): brightness 10 % lifts by 0.2 first, contrast 25 % then
  // doubles the slope around mid-grey (as LightBurn 2.2's adjust-image sample).
  const both = I.adjustImage(ramp, { ...I.DEFAULT_ADJUST, contrast: 25, brightness: 10 }).data;
  near(both[1], 255 * Math.max(0, (64 / 255 + 0.2 - 0.5) * 2 + 0.5), 1e-3);
  near(both[2], 255 * Math.min(1, (128 / 255 + 0.2 - 0.5) * 2 + 0.5), 1e-3);
  assert.deepEqual(I.toneFactors({ contrast: -25, brightness: -10 }), { shift: -0.2, slope: 0.5 });
  const gamma = I.adjustImage(ramp, { ...I.DEFAULT_ADJUST, gamma: 2 }).data;
  assert.ok(gamma[2] < 128, "gamma above 1 darkens the mid-tones (grey^gamma)");
  near(gamma[2], 255 * (128 / 255) ** 2, 1e-3);
  assert.equal(gamma[0], 0);
  assert.equal(gamma[4], 255);
});

test("enhance sharpens edges and denoise leaves small differences alone", () => {
  const edge: I.GrayImage = { width: 8, height: 1, data: Float32Array.from([100, 100, 100, 100, 150, 150, 150, 150]) };
  const sharp = I.adjustImage(edge, { ...I.DEFAULT_ADJUST, enhance_radius: 1, enhance_amount: 100 }).data;
  assert.ok(sharp[3] < 100 && sharp[4] > 150, "overshoot on both sides of the edge");
  assert.equal(sharp[0], 100);
  const quiet = I.adjustImage(edge, { ...I.DEFAULT_ADJUST, enhance_radius: 1, enhance_amount: 100, denoise: 50 }).data;
  assert.deepEqual([...quiet], [...edge.data]);
});

test("every dither mode keeps white white and black black", () => {
  for (const m of I.DITHER_MODES) {
    assert.equal(I.burnShare(I.ditherImage(flat(255), m.value, opts)), 0, `${m.value} white`);
    if (m.value !== "sketch") assert.equal(I.burnShare(I.ditherImage(flat(0), m.value, opts)), 1, `${m.value} black`);
  }
});

test("dithers reproduce the grey level", () => {
  for (const mode of ["floyd_steinberg", "jarvis", "stucki", "ordered"]) {
    for (const level of [0.25, 0.5, 0.75]) near(I.burnShare(I.ditherImage(flat(255 * (1 - level)), mode, opts)), level, 0.03);
  }
  // Atkinson only diffuses 3/4 of the error, so mid-greys come out a little light. Screens cover the grey's share of
  // each cell (a disc of that area), up to aliasing on a coarse grid.
  near(I.burnShare(I.ditherImage(flat(128), "atkinson", opts)), 0.5, 0.08);
  for (const level of [0.2, 0.5]) near(I.burnShare(I.ditherImage(flat(255 * (1 - level), 200, 200), "halftone", opts)), level, 0.03);
  const ht = [0.2, 0.4, 0.6, 0.8].map((l) => I.burnShare(I.ditherImage(flat(255 * (1 - l), 200, 200), "halftone", opts)));
  for (let i = 1; i < ht.length; i++) assert.ok(ht[i] > ht[i - 1], `halftone coverage rises: ${ht}`);
});

test("ordered uses the standard 8x8 Bayer matrix", () => {
  // Row 0 of the matrix: 0 32 8 40 2 34 10 42; a darkness just above (n + 0.5) / 64 fires exactly those dots.
  const img: I.GrayImage = { width: 8, height: 1, data: new Float32Array(8).fill(255 * (1 - 9 / 64)) };
  assert.deepEqual([...I.ditherImage(img, "ordered", opts)].map((v) => (v === 0 ? 1 : 0)), [1, 0, 1, 0, 1, 0, 0, 0]);
});

test("sketch keeps edges only", () => {
  const w = 10;
  const data = new Float32Array(w * w);
  for (let i = 0; i < data.length; i++) data[i] = i % w < 5 ? 0 : 255;
  const out = I.ditherImage({ width: w, height: w, data }, "sketch", opts);
  for (let y = 0; y < w; y++) assert.deepEqual([...out.slice(y * w, y * w + w)].map((v) => (v === 0 ? 1 : 0)), [0, 0, 0, 0, 1, 1, 0, 0, 0, 0]);
});

test("threshold splits at mid-grey and unknown modes fall back to Jarvis", () => {
  const ramp: I.GrayImage = { width: 4, height: 1, data: Float32Array.from([0, 127, 128, 255]) };
  assert.deepEqual([...I.ditherImage(ramp, "threshold", opts)], [0, 0, 255, 255]);
  assert.deepEqual([...I.ditherImage(flat(100), "no-such-mode", opts)], [...I.ditherImage(flat(100), "jarvis", opts)]);
  // LightBurn's own names for Floyd-Steinberg read the same.
  for (const alias of ["dither", "dithered"]) assert.deepEqual([...I.ditherImage(flat(100), alias, opts)], [...I.ditherImage(flat(100), "floyd_steinberg", opts)]);
});

test("grayscale maps darkness between min power and full power, skipping white", () => {
  const ramp: I.GrayImage = { width: 3, height: 1, data: Float32Array.from([255, 254, 0]) };
  assert.deepEqual([...I.ditherImage(ramp, "grayscale", { ...opts, minPowerFraction: 0.5 })], [255, 127, 0]);
  assert.deepEqual([...I.ditherImage({ ...ramp, data: Float32Array.from([128]), width: 1 }, "grayscale", opts)], [128]);
});

test("halftone cells follow cells per inch", () => {
  // 0.05 mm pixels, 25.4 cells per inch = 1 mm cells = 20 px: a 10 % grey makes one dot per cell.
  const out = I.ditherImage(flat(255 * 0.9, 100, 100), "halftone", { ...opts, halftoneCellsPerInch: 25.4, halftoneAngleDeg: 0 });
  // Count connected dots (4-neighbour flood fill).
  const seen = new Uint8Array(out.length);
  let dots = 0;
  for (let i = 0; i < out.length; i++) {
    if (out[i] !== 0 || seen[i]) continue;
    dots++;
    const stack = [i];
    seen[i] = 1;
    while (stack.length) {
      const j = stack.pop()!;
      const [x, y] = [j % 100, Math.floor(j / 100)];
      for (const [nx, ny] of [[x + 1, y], [x - 1, y], [x, y + 1], [x, y - 1]]) {
        const k = ny * 100 + nx;
        if (nx >= 0 && nx < 100 && ny >= 0 && ny < 100 && out[k] === 0 && !seen[k]) {
          seen[k] = 1;
          stack.push(k);
        }
      }
    }
  }
  assert.ok(dots >= 25 && dots <= 36, `about 5x5 dots, got ${dots}`);
});

test("pipeline: negative inverts; pass-through keeps the dither", () => {
  const s = { ...I.DEFAULT_IMAGE_SETTINGS, dither: "threshold" };
  const A = I.DEFAULT_ADJUST;
  const ramp: I.GrayImage = { width: 2, height: 1, data: Float32Array.from([0, 255]) };
  assert.deepEqual([...I.processImage(ramp, s, A, 0.05, 50)], [0, 255]);
  assert.deepEqual([...I.processImage(ramp, { ...s, negative: true }, A, 0.05, 50)], [255, 0]);
  const grey = flat(100, 4, 4);
  const jarvis = { ...s, dither: "jarvis" };
  assert.deepEqual([...I.processImage(grey, { ...jarvis, pass_through: true }, A, 0.05, 50)], [...I.processImage(grey, jarvis, A, 0.05, 50)]);
  // The image's adjustment runs before the dither: brightness +100 % turns this grey white.
  assert.deepEqual([...I.processImage(grey, s, { ...A, brightness: 100 }, 0.05, 50)], new Array(16).fill(255));
  // Grayscale min power is relative to the layer's power.
  const g = I.processImage({ width: 1, height: 1, data: Float32Array.from([254]) }, { ...s, dither: "grayscale", min_power_pct: 25 }, A, 0.05, 50);
  near(g[0], 127, 1);
  assert.deepEqual([...I.grayToRgba(Uint8ClampedArray.from([0, 200]))], [0, 0, 0, 255, 200, 200, 200, 255]);
});

test("canvas view store notifies on changes only", () => {
  let n = 0;
  const off = I.imageView.subscribe(() => n++);
  const start = I.imageView.get().view;
  I.imageView.setView(start);
  assert.equal(n, 0);
  I.imageView.setView(start === "original" ? "dithered" : "original");
  assert.equal(n, 1);
  I.imageView.setDraft("o1", I.DEFAULT_ADJUST);
  assert.equal(I.imageView.get().drafts.get("o1"), I.DEFAULT_ADJUST);
  I.imageView.setDraft("o1", null);
  I.imageView.setDraft("o1", null);
  assert.equal(n, 3);
  off();
  I.imageView.setView(start);
  assert.equal(n, 3);
});

test("Adjust Image presets: list, picking one and the saved body", () => {
  const list = I.imagePresetList({
    presets: [
      { name: "z", adjust: { gamma: 1.5 } },
      { name: "a", image: { dither: "stucki" } },
      { name: "empty" },
      { adjust: { gamma: 2 } },
    ],
  });
  assert.deepEqual(list.map((p) => p.name), ["a", "z"]);
  const cur = { ...I.DEFAULT_ADJUST, contrast: 20 };
  assert.deepEqual(I.adjustFromPreset(list[1]!, cur), { ...I.DEFAULT_ADJUST, gamma: 1.5 }, "a preset's adjustments replace all six");
  assert.equal(I.adjustFromPreset(list[0]!, cur), cur, "a layer-only preset keeps the adjustments");
  assert.deepEqual(I.imagePresetBody(cur, null), { adjust: cur });
  assert.deepEqual(Object.keys(I.imagePresetBody(cur, I.DEFAULT_IMAGE_SETTINGS)), ["adjust", "image"]);
  assert.deepEqual(I.imagePresetList(null), []);
});
