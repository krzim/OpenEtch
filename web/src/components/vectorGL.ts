// The document's vectors on the GPU (WebGL2), the way toolpathGL.ts draws the preview: the scene's segments
// (vectorScene.ts) are uploaded once per document and every frame is a handful of instanced draw calls, whatever the
// number of objects or points. Each segment is a quad of constant device-px width with square ends half the width long
// (so polyline joins close); a stencil pass per translucent group keeps it from darkening where its own segments
// overlap, as one SVG path would. Fills (the filled view) are even-odd by stencil: one triangle per segment from its
// subpath's first point inverts the stencil, then a full-screen cover draws where it is odd, so no triangulation is
// needed. Selection and the outside-workspace marks are per-segment flags (re-uploaded when they change); the
// transient transform of a drag, the dash phase of the marching ants and the view are uniforms. Callers fall back to
// the 2D canvas (VectorLayer.tsx) when WebGL2 is missing.

import type { Affine } from "../api/types.ts";
import type { View } from "../sceneLayers.ts";
import { SEG_STRIDE, type VectorScene } from "../vectorScene.ts";

export type RGBA = [number, number, number, number];

/** One layer group's colours (straight RGBA, 0..1), from vectorScene.ts groupLook. */
export interface GroupDraw {
  stroke: RGBA;
  fill: RGBA | null;
  movingFill: RGBA | null;
}

export interface VectorFrame {
  groups: GroupDraw[];
  /** The transient transform of a drag, applied to the selected objects; null when nothing moves. */
  live: Affine | null;
  objWidth: number;
  sel: { rgba: RGBA; width: number; dash: [number, number]; phase: number } | null;
  outside: { rgba: RGBA; width: number } | null;
}

const COMMON = `#version 300 es
in vec4 a_seg;
in vec4 a_meta;
in float a_flags;
uniform vec2 u_center;
uniform float u_scale;
uniform vec2 u_half;
uniform int u_liveOn;
uniform vec4 u_liveM;
uniform vec2 u_liveT;
vec2 live(vec2 p) { return mat2(u_liveM.xy, u_liveM.zw) * p + u_liveT; }
vec2 toDev(vec2 p) { return (p - u_center) * u_scale; }
`;

const VS_STROKE = `${COMMON}
uniform float u_width;
uniform int u_pass;
out float v_along;
void main() {
  int meta = int(a_meta.w + 0.5);
  int flags = int(a_flags + 0.5);
  bool sel = (flags & 1) != 0;
  bool keep = (meta & 1) == 0 && (u_pass == 0 || (u_pass == 1 && sel) || (u_pass == 2 && (flags & 2) != 0));
  if (!keep) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_along = 0.0;
    return;
  }
  vec2 A = a_seg.xy;
  vec2 B = a_seg.zw;
  float k = 1.0;
  if (u_liveOn == 1 && sel) {
    A = live(A);
    B = live(B);
    k = sqrt(abs(u_liveM.x * u_liveM.w - u_liveM.y * u_liveM.z));
  }
  vec2 a = toDev(A);
  vec2 b = toDev(B);
  vec2 d = b - a;
  float len = length(d);
  vec2 dir = len > 1e-6 ? d / len : vec2(1.0, 0.0);
  vec2 nrm = vec2(-dir.y, dir.x);
  float hw = u_width * 0.5;
  float cx = float(gl_VertexID & 1);
  float cy = float(gl_VertexID >> 1) * 2.0 - 1.0;
  vec2 p = mix(a - dir * hw, b + dir * hw, cx) + nrm * (hw * cy);
  v_along = a_meta.x * u_scale * k + mix(-hw, len + hw, cx);
  gl_Position = vec4(p / u_half, 0.0, 1.0);
}`;

const FS_STROKE = `#version 300 es
precision highp float;
uniform vec4 u_color;
uniform vec3 u_dash;
in float v_along;
out vec4 o_color;
void main() {
  if (u_dash.x > 0.0 && mod(v_along - u_dash.z, u_dash.x + u_dash.y) > u_dash.x) discard;
  o_color = u_color;
}`;

const VS_FILL = `${COMMON}
uniform int u_moving;
void main() {
  int meta = int(a_meta.w + 0.5);
  int flags = int(a_flags + 0.5);
  bool moving = u_liveOn == 1 && (flags & 1) != 0;
  if ((meta & 2) != 0 || moving != (u_moving == 1)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    return;
  }
  vec2 P = gl_VertexID == 0 ? a_meta.yz : (gl_VertexID == 1 ? a_seg.xy : a_seg.zw);
  if (moving) P = live(P);
  gl_Position = vec4(toDev(P) / u_half, 0.0, 1.0);
}`;

const VS_COVER = `#version 300 es
void main() {
  gl_Position = vec4(float((gl_VertexID & 1) * 4 - 1), float((gl_VertexID >> 1) * 4 - 1), 0.0, 1.0);
}`;

const FS_FLAT = `#version 300 es
precision highp float;
uniform vec4 u_color;
out vec4 o_color;
void main() {
  o_color = u_color;
}`;

type Loc = Record<string, WebGLUniformLocation | null>;
interface Prog {
  p: WebGLProgram;
  u: Loc;
  seg: number;
  meta: number;
  flags: number;
}

const UNIFORMS = ["u_center", "u_scale", "u_half", "u_liveOn", "u_liveM", "u_liveT", "u_width", "u_pass", "u_color", "u_dash", "u_moving"];

export class VectorGL {
  private gl: WebGL2RenderingContext;
  private stroke: Prog | null = null;
  private fill: Prog | null = null;
  private cover: Prog | null = null;
  private vao: WebGLVertexArrayObject | null = null;
  /** No attributes: the cover pass reads none. */
  private emptyVao: WebGLVertexArrayObject | null = null;
  private segBuf: WebGLBuffer | null = null;
  private flagBuf: WebGLBuffer | null = null;
  private scene: VectorScene | null = null;
  private flags: Uint8Array | null = null;
  /** Per group: holds a selected segment (its moving fill is drawn separately during a drag). */
  private groupSel: boolean[] = [];
  lost = false;
  private canvas: HTMLCanvasElement;
  private onLostChange: ((lost: boolean) => void) | undefined;

  /**
   * Throws when WebGL2 is not available. `onLostChange` hears when the context is lost (true: nothing reaches the
   * screen until it is restored, so draw another way meanwhile) and restored (false: everything is uploaded again,
   * draw now).
   */
  constructor(canvas: HTMLCanvasElement, onLostChange?: (lost: boolean) => void) {
    this.canvas = canvas;
    this.onLostChange = onLostChange;
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
    this.segBuf = null;
    this.flagBuf = null;
    this.onLostChange?.(true);
  };

  private onRestored = () => {
    this.lost = false;
    this.init();
    const s = this.scene;
    const f = this.flags;
    this.scene = null;
    if (s) this.setScene(s);
    if (f) this.setFlags(f);
    this.onLostChange?.(false);
  };

  private program(vs: string, fs: string): Prog {
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
    // The same locations in every program, so the shared VAO's enabled arrays always mean the same attribute.
    gl.bindAttribLocation(p, 0, "a_seg");
    gl.bindAttribLocation(p, 1, "a_meta");
    gl.bindAttribLocation(p, 2, "a_flags");
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
    const u: Loc = {};
    for (const n of UNIFORMS) u[n] = gl.getUniformLocation(p, n);
    return { p, u, seg: gl.getAttribLocation(p, "a_seg"), meta: gl.getAttribLocation(p, "a_meta"), flags: gl.getAttribLocation(p, "a_flags") };
  }

  private init(): void {
    const gl = this.gl;
    this.stroke = this.program(VS_STROKE, FS_STROKE);
    this.fill = this.program(VS_FILL, FS_FLAT);
    this.cover = this.program(VS_COVER, FS_FLAT);
    this.vao = gl.createVertexArray();
    this.emptyVao = gl.createVertexArray();
    this.segBuf = gl.createBuffer();
    this.flagBuf = gl.createBuffer();
  }

  /** Uploads the scene's segments (once per document). */
  setScene(scene: VectorScene | null): void {
    if (scene === this.scene) return;
    this.scene = scene;
    this.flags = null;
    this.groupSel = [];
    if (this.lost || !this.segBuf) return;
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.segBuf);
    gl.bufferData(gl.ARRAY_BUFFER, scene ? scene.segs : new Float32Array(0), gl.STATIC_DRAW);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.flagBuf);
    gl.bufferData(gl.ARRAY_BUFFER, new Uint8Array(Math.max(1, scene?.count ?? 0)), gl.DYNAMIC_DRAW);
  }

  /** Uploads the per-segment flags (when the selection or the outside set changes). */
  setFlags(flags: Uint8Array): void {
    this.flags = flags;
    const s = this.scene;
    this.groupSel = s ? s.groups.map((g) => flags.subarray(g.first, g.first + g.count).some((f) => (f & 1) !== 0)) : [];
    if (this.lost || !this.flagBuf || !s || flags.length !== s.count) return;
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.flagBuf);
    gl.bufferData(gl.ARRAY_BUFFER, flags.length ? flags : new Uint8Array(1), gl.DYNAMIC_DRAW);
  }

  /** Points the instanced attributes of `p` at segments first.. (WebGL2 has no base instance). */
  private bind(p: Prog, first: number): void {
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.segBuf);
    if (p.seg >= 0) {
      gl.enableVertexAttribArray(p.seg);
      gl.vertexAttribPointer(p.seg, 4, gl.FLOAT, false, SEG_STRIDE * 4, first * SEG_STRIDE * 4);
      gl.vertexAttribDivisor(p.seg, 1);
    }
    if (p.meta >= 0) {
      gl.enableVertexAttribArray(p.meta);
      gl.vertexAttribPointer(p.meta, 4, gl.FLOAT, false, SEG_STRIDE * 4, first * SEG_STRIDE * 4 + 16);
      gl.vertexAttribDivisor(p.meta, 1);
    }
    if (p.flags >= 0) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.flagBuf);
      gl.enableVertexAttribArray(p.flags);
      gl.vertexAttribPointer(p.flags, 1, gl.UNSIGNED_BYTE, false, 1, first);
      gl.vertexAttribDivisor(p.flags, 1);
    }
  }

  private common(p: Prog, view: View, dpr: number, w: number, h: number, live: Affine | null): void {
    const gl = this.gl;
    gl.useProgram(p.p);
    gl.uniform2f(p.u.u_center, view.cx, view.cy);
    gl.uniform1f(p.u.u_scale, view.scale * dpr);
    gl.uniform2f(p.u.u_half, w / 2, h / 2);
    gl.uniform1i(p.u.u_liveOn, live ? 1 : 0);
    const m = live ?? [1, 0, 0, 1, 0, 0];
    gl.uniform4f(p.u.u_liveM, m[0], m[1], m[2], m[3]);
    gl.uniform2f(p.u.u_liveT, m[4], m[5]);
  }

  private color(p: Prog, c: RGBA): void {
    this.gl.uniform4f(p.u.u_color, c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]);
  }

  /** Draws the scene for `view` on a canvas `size` CSS px large at `dpr`. */
  draw(view: View, size: [number, number], dpr: number, f: VectorFrame): void {
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
    gl.stencilMask(0xff);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.STENCIL_BUFFER_BIT);
    const s = this.scene;
    const st = this.stroke;
    const fl = this.fill;
    const cv = this.cover;
    if (this.lost || !s || !s.count || !st || !fl || !cv || !this.flags || this.flags.length !== s.count) return;
    gl.bindVertexArray(this.vao);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    // Fills: per group, static objects, then (during a drag) the moving ones under the live transform.
    const fillPass = (first: number, count: number, rgba: RGBA, moving: boolean) => {
      gl.clear(gl.STENCIL_BUFFER_BIT);
      gl.enable(gl.STENCIL_TEST);
      gl.colorMask(false, false, false, false);
      gl.stencilFunc(gl.ALWAYS, 0, 1);
      gl.stencilOp(gl.KEEP, gl.KEEP, gl.INVERT);
      this.common(fl, view, dpr, w, h, f.live);
      gl.uniform1i(fl.u.u_moving, moving ? 1 : 0);
      this.bind(fl, first);
      gl.drawArraysInstanced(gl.TRIANGLES, 0, 3, count);
      gl.colorMask(true, true, true, true);
      gl.stencilFunc(gl.EQUAL, 1, 1);
      gl.stencilOp(gl.ZERO, gl.ZERO, gl.ZERO);
      gl.useProgram(cv.p);
      this.color(cv, rgba);
      gl.bindVertexArray(this.emptyVao);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      gl.bindVertexArray(this.vao);
      gl.disable(gl.STENCIL_TEST);
    };
    s.groups.forEach((g, i) => {
      const d = f.groups[i];
      if (!d || !g.count) return;
      if (d.fill && d.fill[3] > 0) fillPass(g.first, g.count, d.fill, false);
      if (f.live && d.movingFill && d.movingFill[3] > 0 && this.groupSel[i]) fillPass(g.first, g.count, d.movingFill, true);
    });

    // Strokes: translucent ones blend once per pixel per pass.
    const strokePass = (first: number, count: number, rgba: RGBA, width: number, pass: number, dash: [number, number] | null, phase: number) => {
      if (rgba[3] <= 0) return;
      if (rgba[3] < 1) {
        gl.clear(gl.STENCIL_BUFFER_BIT);
        gl.enable(gl.STENCIL_TEST);
        gl.stencilFunc(gl.EQUAL, 0, 0xff);
        gl.stencilOp(gl.KEEP, gl.KEEP, gl.INCR);
      } else gl.disable(gl.STENCIL_TEST);
      this.common(st, view, dpr, w, h, f.live);
      gl.uniform1f(st.u.u_width, width * dpr);
      gl.uniform1i(st.u.u_pass, pass);
      gl.uniform3f(st.u.u_dash, dash ? dash[0] * dpr : 0, dash ? dash[1] * dpr : 0, phase * dpr);
      this.color(st, rgba);
      this.bind(st, first);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    };
    s.groups.forEach((g, i) => {
      const d = f.groups[i];
      if (d && g.count) strokePass(g.first, g.count, d.stroke, f.objWidth, 0, null, 0);
    });
    if (f.sel) strokePass(0, s.count, f.sel.rgba, f.sel.width, 1, f.sel.dash, f.sel.phase);
    if (f.outside) strokePass(0, s.count, f.outside.rgba, f.outside.width, 2, null, 0);
    gl.disable(gl.STENCIL_TEST);
    gl.bindVertexArray(null);
  }

  dispose(): void {
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
    this.canvas.removeEventListener("webglcontextrestored", this.onRestored);
    this.onLostChange = undefined;
    const gl = this.gl;
    releaseWhenDetached(this.canvas, gl);
    if (this.lost) return;
    if (this.segBuf) gl.deleteBuffer(this.segBuf);
    if (this.flagBuf) gl.deleteBuffer(this.flagBuf);
    if (this.vao) gl.deleteVertexArray(this.vao);
    if (this.emptyVao) gl.deleteVertexArray(this.emptyVao);
    for (const p of [this.stroke, this.fill, this.cover]) if (p) gl.deleteProgram(p.p);
  }
}

/**
 * Frees a disposed renderer's context once its canvas has left the page, instead of when the canvas is collected.
 * Browsers keep about 16 WebGL contexts and lose the oldest beyond that, which would be the canvas's long-lived vector
 * layer after the preview layer was shown and hidden a few times. A canvas still in the page (an effect run again on
 * the same element) keeps its context.
 */
export function releaseWhenDetached(canvas: HTMLCanvasElement, gl: WebGL2RenderingContext): void {
  setTimeout(() => {
    if (canvas.isConnected || gl.isContextLost()) return;
    gl.getExtension("WEBGL_lose_context")?.loseContext();
  }, 0);
}
