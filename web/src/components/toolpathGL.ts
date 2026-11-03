// The toolpath preview on the GPU (WebGL2): every segment is one instanced quad, so a frame costs one draw call per
// batch however many segments there are (a 220k-run image preview strokes in well under a millisecond, where a 2D
// canvas stroke of the same paths takes hundreds). Widths are in device px (constant on screen), marks get square
// caps half their width long (so single-dot runs stay visible, like the round caps of the SVG preview), travel is
// dashed in the fragment shader by the distance along each segment, and a stencil pass per batch keeps translucent
// (energy-shaded) batches from darkening where their own segments overlap, as one SVG path would. Callers fall back
// to the 2D canvas (SceneCanvas.tsx paintToolpath) when WebGL2 is missing.
//
// Image passes come as rasters (api/packedPreview.ts PreviewRaster) and are drawn as textures: one quad per raster
// (per tile of it past the GPU's texture size) with each dot's shade (its power against the pass's) in an R8 texture,
// uploaded once per preview however the slider moves. Crisp dots when magnified (nearest), mipmapped when shrunk; the
// slider's position shows whole rows and the done part of the current one (cut in the fragment shader).

import { levelShades, type PreviewRaster } from "../api/packedPreview.ts";
import type { Toolpath, ToolpathRaster } from "../sceneLayers.ts";
import type { View } from "../sceneLayers.ts";
import { releaseWhenDetached } from "./vectorGL.ts";

export interface BatchStyle {
  /** Straight (not premultiplied) RGBA, 0..1. */
  rgba: [number, number, number, number];
  /** Line width, CSS px. */
  widthPx: number;
  /** Dash on/off lengths in CSS px, or null for solid. */
  dash: [number, number] | null;
  /** Square caps half the width long. */
  caps: boolean;
}

const VS = `#version 300 es
in vec2 a_corner;
in vec4 a_seg;
uniform vec2 u_center;
uniform float u_scale;
uniform vec2 u_half;
uniform float u_width;
uniform float u_cap;
out float v_along;
void main() {
  vec2 a = (a_seg.xy - u_center) * u_scale;
  vec2 b = (a_seg.zw - u_center) * u_scale;
  vec2 d = b - a;
  float len = length(d);
  vec2 dir = len > 1e-6 ? d / len : vec2(1.0, 0.0);
  vec2 nrm = vec2(-dir.y, dir.x);
  float hw = u_width * 0.5;
  float ext = u_cap * hw;
  vec2 p = mix(a - dir * ext, b + dir * ext, a_corner.x) + nrm * (hw * a_corner.y);
  v_along = mix(-ext, len + ext, a_corner.x);
  gl_Position = vec4(p / u_half, 0.0, 1.0);
}`;

const FS = `#version 300 es
precision highp float;
uniform vec4 u_color;
uniform vec2 u_dash;
in float v_along;
out vec4 o_color;
void main() {
  if (u_dash.x > 0.0 && mod(v_along, u_dash.x + u_dash.y) > u_dash.x) discard;
  o_color = u_color;
}`;

// A raster tile's quad: grid units (column, row) to field mm through the raster's affine, then to the screen as above.
const RASTER_VS = `#version 300 es
in vec2 a_corner;
uniform vec2 u_center;
uniform float u_scale;
uniform vec2 u_half;
uniform vec4 u_abcd;
uniform vec2 u_ef;
uniform vec4 u_tile;
out vec2 v_grid;
void main() {
  vec2 g = u_tile.xy + vec2(a_corner.x, a_corner.y * 0.5 + 0.5) * u_tile.zw;
  v_grid = g;
  vec2 f = vec2(u_abcd.x * g.x + u_abcd.z * g.y, u_abcd.y * g.x + u_abcd.w * g.y) + u_ef;
  gl_Position = vec4((f - u_center) * u_scale / u_half, 0.0, 1.0);
}`;

// Dots past the slider are left out: rows from `u_rows` on, except the partial row between its two columns.
const RASTER_FS = `#version 300 es
precision highp float;
uniform sampler2D u_tex;
uniform vec4 u_color;
uniform vec4 u_tile;
uniform float u_rows;
uniform vec3 u_partial;
in vec2 v_grid;
out vec4 o_color;
void main() {
  float row = floor(v_grid.y);
  if (row >= u_rows && (row != u_partial.x || v_grid.x < min(u_partial.y, u_partial.z) || v_grid.x > max(u_partial.y, u_partial.z))) discard;
  float s = texture(u_tex, (v_grid - u_tile.xy) / u_tile.zw).r;
  if (s <= 0.0) discard;
  o_color = u_color * s;
}`;

interface Uploaded {
  key: string;
  buffer: WebGLBuffer;
  count: number;
}

/** One texture of a raster: dots `c0..c0 + w` x `r0..r0 + h`. */
interface RasterTile {
  tex: WebGLTexture;
  c0: number;
  r0: number;
  w: number;
  h: number;
}

interface Program {
  program: WebGLProgram;
  loc: Record<string, WebGLUniformLocation | null>;
}

/** Largest texture side used (the GPU's own limit when that is smaller). */
const MAX_TILE = 8192;

/** Each dot's shade (0 unmarked) of raster `r`, row by row. */
export function rasterShades(r: PreviewRaster): Uint8Array {
  const lut = levelShades(r);
  const out = new Uint8Array(r.levels.length);
  const lv = r.levels;
  for (let i = 0; i < out.length; i++) out[i] = lut[lv[i]];
  return out;
}

export class ToolpathGL {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  private rasterVao: WebGLVertexArrayObject | null = null;
  private rasterProgram: Program | null = null;
  private corners: WebGLBuffer | null = null;
  private uploaded: Uploaded[] = [];
  private data: Toolpath | null = null;
  private loc: Record<string, WebGLUniformLocation | null> = {};
  private segAttr = -1;
  /** Textures per raster, kept while the preview holds it (the slider only changes how much of it is drawn). */
  private textures = new Map<PreviewRaster, RasterTile[]>();
  lost = false;

  /** Throws when WebGL2 is not available. */
  constructor(private canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { antialias: true, stencil: true, premultipliedAlpha: true, alpha: true });
    if (!gl) throw new Error("no WebGL2");
    this.gl = gl;
    canvas.addEventListener("webglcontextlost", this.onLost);
    canvas.addEventListener("webglcontextrestored", this.onRestored);
    this.init();
  }

  private onLost = (e: Event) => {
    e.preventDefault();
    this.lost = true;
    this.uploaded = [];
    this.textures.clear();
  };

  private onRestored = () => {
    this.lost = false;
    this.init();
    if (this.data) this.setData(this.data);
  };

  private link(vs: string, fs: string, uniforms: string[]): Program {
    const gl = this.gl;
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type) as WebGLShader;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
      return s;
    };
    const p = gl.createProgram() as WebGLProgram;
    gl.attachShader(p, sh(gl.VERTEX_SHADER, vs));
    gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
    const loc: Record<string, WebGLUniformLocation | null> = {};
    for (const u of uniforms) loc[u] = gl.getUniformLocation(p, u);
    return { program: p, loc };
  }

  private init(): void {
    const gl = this.gl;
    const seg = this.link(VS, FS, ["u_center", "u_scale", "u_half", "u_width", "u_cap", "u_color", "u_dash"]);
    const p = seg.program;
    this.program = p;
    this.loc = seg.loc;
    this.vao = gl.createVertexArray();
    gl.bindVertexArray(this.vao);
    this.corners = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.corners);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, -1, 1, -1, 0, 1, 1, 1]), gl.STATIC_DRAW);
    const corner = gl.getAttribLocation(p, "a_corner");
    gl.enableVertexAttribArray(corner);
    gl.vertexAttribPointer(corner, 2, gl.FLOAT, false, 0, 0);
    this.segAttr = gl.getAttribLocation(p, "a_seg");
    gl.enableVertexAttribArray(this.segAttr);
    gl.vertexAttribDivisor(this.segAttr, 1);
    // Rasters: the same corner strip, one quad per tile.
    this.rasterProgram = this.link(RASTER_VS, RASTER_FS, ["u_center", "u_scale", "u_half", "u_abcd", "u_ef", "u_tile", "u_tex", "u_color", "u_rows", "u_partial"]);
    this.rasterVao = gl.createVertexArray();
    gl.bindVertexArray(this.rasterVao);
    const rc = gl.getAttribLocation(this.rasterProgram.program, "a_corner");
    gl.enableVertexAttribArray(rc);
    gl.vertexAttribPointer(rc, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
  }

  /** Uploads the segments (once per preview and slider position) and the rasters' textures (once per preview). */
  setData(tp: Toolpath | null): void {
    const gl = this.gl;
    for (const u of this.uploaded) gl.deleteBuffer(u.buffer);
    this.uploaded = [];
    this.data = tp;
    if (this.lost) return;
    // Textures of rasters no longer shown go; the others stay for the next slider position.
    const keep = new Set((tp?.rasters ?? []).map((r) => r.raster));
    for (const [r, tiles] of this.textures) {
      if (keep.has(r)) continue;
      for (const t of tiles) gl.deleteTexture(t.tex);
      this.textures.delete(r);
    }
    if (!tp) return;
    for (const b of tp.batches) {
      const buffer = gl.createBuffer() as WebGLBuffer;
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(gl.ARRAY_BUFFER, b.coords, gl.STATIC_DRAW);
      this.uploaded.push({ key: b.key, buffer, count: b.coords.length / 4 });
    }
    for (const r of tp.rasters) this.texturesOf(r.raster);
  }

  /** The textures of raster `r`: each dot's shade, in tiles the GPU takes, mipmapped. */
  private texturesOf(r: PreviewRaster): RasterTile[] {
    const have = this.textures.get(r);
    if (have) return have;
    const gl = this.gl;
    const shades = rasterShades(r);
    const side = Math.min(MAX_TILE, (gl.getParameter(gl.MAX_TEXTURE_SIZE) as number) || 4096);
    const tiles: RasterTile[] = [];
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, r.cols);
    for (let r0 = 0; r0 < r.rows; r0 += side) {
      for (let c0 = 0; c0 < r.cols; c0 += side) {
        const w = Math.min(side, r.cols - c0);
        const h = Math.min(side, r.rows - r0);
        const tex = gl.createTexture() as WebGLTexture;
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, c0);
        gl.pixelStorei(gl.UNPACK_SKIP_ROWS, r0);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, w, h, 0, gl.RED, gl.UNSIGNED_BYTE, shades);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        gl.generateMipmap(gl.TEXTURE_2D);
        tiles.push({ tex, c0, r0, w, h });
      }
    }
    gl.pixelStorei(gl.UNPACK_ROW_LENGTH, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_PIXELS, 0);
    gl.pixelStorei(gl.UNPACK_SKIP_ROWS, 0);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.bindTexture(gl.TEXTURE_2D, null);
    this.textures.set(r, tiles);
    return tiles;
  }

  /** Draws every batch and raster for `view` on a canvas `size` CSS px large at `dpr`. */
  draw(view: View, size: [number, number], dpr: number, styleOf: (key: string) => BatchStyle | null): void {
    const gl = this.gl;
    const w = Math.max(1, Math.round(size[0] * dpr));
    const h = Math.max(1, Math.round(size[1] * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    gl.clearColor(0, 0, 0, 0);
    gl.clearStencil(0);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.STENCIL_BUFFER_BIT);
    const rasters = this.data?.rasters ?? [];
    if (this.lost || !this.program || (this.uploaded.length === 0 && rasters.length === 0)) return;
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    // Travel first, then the images, then the vector marks over them.
    const at = { view, w, h, dpr };
    this.drawBatches(
      this.uploaded.filter((u) => u.key === "jump"),
      at,
      styleOf,
    );
    this.drawRasters(rasters, at, styleOf);
    this.drawBatches(
      this.uploaded.filter((u) => u.key !== "jump"),
      at,
      styleOf,
    );
  }

  private drawBatches(batches: Uploaded[], at: { view: View; w: number; h: number; dpr: number }, styleOf: (key: string) => BatchStyle | null): void {
    const gl = this.gl;
    if (batches.length === 0 || !this.program) return;
    const { view, w, h, dpr } = at;
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.enable(gl.STENCIL_TEST);
    gl.stencilFunc(gl.EQUAL, 0, 0xff);
    gl.stencilOp(gl.KEEP, gl.KEEP, gl.INCR);
    gl.uniform2f(this.loc.u_center, view.cx, view.cy);
    gl.uniform1f(this.loc.u_scale, view.scale * dpr);
    gl.uniform2f(this.loc.u_half, w / 2, h / 2);
    for (const u of batches) {
      const st = styleOf(u.key);
      if (!st || st.rgba[3] <= 0) continue;
      const [r, g, b, a] = st.rgba;
      gl.uniform4f(this.loc.u_color, r * a, g * a, b * a, a);
      gl.uniform1f(this.loc.u_width, st.widthPx * dpr);
      gl.uniform1f(this.loc.u_cap, st.caps ? 1 : 0);
      gl.uniform2f(this.loc.u_dash, st.dash ? st.dash[0] * dpr : 0, st.dash ? st.dash[1] * dpr : 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, u.buffer);
      gl.vertexAttribPointer(this.segAttr, 4, gl.FLOAT, false, 0, 0);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, u.count);
      // Each batch blends once per pixel: start the next one with a clean stencil.
      gl.clear(gl.STENCIL_BUFFER_BIT);
    }
    gl.disable(gl.STENCIL_TEST);
    gl.bindVertexArray(null);
  }

  private drawRasters(rasters: readonly ToolpathRaster[], at: { view: View; w: number; h: number; dpr: number }, styleOf: (key: string) => BatchStyle | null): void {
    const gl = this.gl;
    const rp = this.rasterProgram;
    if (!rp || rasters.length === 0) return;
    const { view, w, h, dpr } = at;
    const loc = rp.loc;
    gl.useProgram(rp.program);
    gl.bindVertexArray(this.rasterVao);
    gl.uniform2f(loc.u_center, view.cx, view.cy);
    gl.uniform1f(loc.u_scale, view.scale * dpr);
    gl.uniform2f(loc.u_half, w / 2, h / 2);
    gl.uniform1i(loc.u_tex, 0);
    gl.activeTexture(gl.TEXTURE0);
    for (const tr of rasters) {
      const st = styleOf(tr.key);
      if (!st || st.rgba[3] <= 0 || (tr.rows === 0 && !tr.partial)) continue;
      const [a, b, c, d, e, f] = tr.raster.grid;
      const [cr, cg, cb, ca] = st.rgba;
      gl.uniform4f(loc.u_color, cr * ca, cg * ca, cb * ca, ca);
      gl.uniform4f(loc.u_abcd, a, b, c, d);
      gl.uniform2f(loc.u_ef, e, f);
      gl.uniform1f(loc.u_rows, tr.rows);
      gl.uniform3f(loc.u_partial, tr.partial ? tr.rows : -1, tr.partial?.from ?? 0, tr.partial?.to ?? 0);
      // Only the tiles of rows that show.
      const shown = tr.rows + (tr.partial ? 1 : 0);
      for (const t of this.texturesOf(tr.raster)) {
        if (t.r0 >= shown) continue;
        gl.bindTexture(gl.TEXTURE_2D, t.tex);
        gl.uniform4f(loc.u_tile, t.c0, t.r0, t.w, t.h);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      }
    }
    gl.bindTexture(gl.TEXTURE_2D, null);
    gl.bindVertexArray(null);
  }

  dispose(): void {
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onRestored);
    // The preview layer comes and goes with the preview: free its context rather than let them pile up.
    releaseWhenDetached(this.canvas, this.gl);
    if (this.lost) return;
    const gl = this.gl;
    for (const u of this.uploaded) gl.deleteBuffer(u.buffer);
    this.uploaded = [];
    for (const tiles of this.textures.values()) for (const t of tiles) gl.deleteTexture(t.tex);
    this.textures.clear();
    if (this.corners) gl.deleteBuffer(this.corners);
    if (this.vao) gl.deleteVertexArray(this.vao);
    if (this.rasterVao) gl.deleteVertexArray(this.rasterVao);
    if (this.program) gl.deleteProgram(this.program);
    if (this.rasterProgram) gl.deleteProgram(this.rasterProgram.program);
  }
}
