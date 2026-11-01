// The toolpath preview on the GPU (WebGL2): every segment is one instanced quad, so a frame costs one draw call per
// batch however many segments there are (a 220k-run image preview strokes in well under a millisecond, where a 2D
// canvas stroke of the same paths takes hundreds). Widths are in device px (constant on screen), marks get square
// caps half their width long (so single-dot runs stay visible, like the round caps of the SVG preview), travel is
// dashed in the fragment shader by the distance along each segment, and a stencil pass per batch keeps translucent
// (energy-shaded) batches from darkening where their own segments overlap, as one SVG path would. Callers fall back
// to the 2D canvas (SceneCanvas.tsx paintToolpath) when WebGL2 is missing.

import type { Toolpath } from "../sceneLayers.ts";
import type { View } from "../sceneLayers.ts";

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

interface Uploaded {
  key: string;
  buffer: WebGLBuffer;
  count: number;
}

export class ToolpathGL {
  private gl: WebGL2RenderingContext;
  private program: WebGLProgram | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  private corners: WebGLBuffer | null = null;
  private uploaded: Uploaded[] = [];
  private data: Toolpath | null = null;
  private loc: Record<string, WebGLUniformLocation | null> = {};
  private segAttr = -1;
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
  };

  private onRestored = () => {
    this.lost = false;
    this.init();
    if (this.data) this.setData(this.data);
  };

  private init(): void {
    const gl = this.gl;
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type) as WebGLShader;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
      return s;
    };
    const p = gl.createProgram() as WebGLProgram;
    gl.attachShader(p, sh(gl.VERTEX_SHADER, VS));
    gl.attachShader(p, sh(gl.FRAGMENT_SHADER, FS));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
    this.program = p;
    for (const u of ["u_center", "u_scale", "u_half", "u_width", "u_cap", "u_color", "u_dash"]) this.loc[u] = gl.getUniformLocation(p, u);
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
    gl.bindVertexArray(null);
  }

  /** Uploads the segments (once per preview). */
  setData(tp: Toolpath | null): void {
    const gl = this.gl;
    for (const u of this.uploaded) gl.deleteBuffer(u.buffer);
    this.uploaded = [];
    this.data = tp;
    if (!tp || this.lost) return;
    for (const b of tp.batches) {
      const buffer = gl.createBuffer() as WebGLBuffer;
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(gl.ARRAY_BUFFER, b.coords, gl.STATIC_DRAW);
      this.uploaded.push({ key: b.key, buffer, count: b.coords.length / 4 });
    }
  }

  /** Draws every batch for `view` on a canvas `size` CSS px large at `dpr`. */
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
    if (this.lost || !this.program || this.uploaded.length === 0) return;
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.STENCIL_TEST);
    gl.stencilFunc(gl.EQUAL, 0, 0xff);
    gl.stencilOp(gl.KEEP, gl.KEEP, gl.INCR);
    gl.uniform2f(this.loc.u_center, view.cx, view.cy);
    gl.uniform1f(this.loc.u_scale, view.scale * dpr);
    gl.uniform2f(this.loc.u_half, w / 2, h / 2);
    for (const u of this.uploaded) {
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

  dispose(): void {
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onRestored);
    if (this.lost) return;
    const gl = this.gl;
    for (const u of this.uploaded) gl.deleteBuffer(u.buffer);
    this.uploaded = [];
    if (this.corners) gl.deleteBuffer(this.corners);
    if (this.vao) gl.deleteVertexArray(this.vao);
    if (this.program) gl.deleteProgram(this.program);
  }
}
