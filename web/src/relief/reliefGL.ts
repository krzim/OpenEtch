// The 3D relief preview's renderer (WebGL2, like the toolpath layer in components/toolpathGL.ts): the height field as
// one indexed triangle mesh, lit from the upper left, coloured from the surface to the deepest point. The grid's
// positions and triangles are uploaded once per grid; scrubbing only replaces the depth and normal buffers.

import type { ReliefField } from "./reliefModel.ts";

const VS = `#version 300 es
in vec2 a_pos;
in float a_depth;
in vec3 a_normal;
uniform mat4 u_mvp;
uniform float u_exag;
uniform float u_max;
out vec3 v_n;
out float v_d;
void main() {
  v_n = a_normal;
  v_d = u_max > 0.0 ? a_depth / u_max : 0.0;
  gl_Position = u_mvp * vec4(a_pos, -a_depth * u_exag, 1.0);
}`;

const FS = `#version 300 es
precision highp float;
in vec3 v_n;
in float v_d;
uniform vec3 u_light;
uniform vec3 u_top;
uniform vec3 u_deep;
uniform float u_ramp;
out vec4 o_color;
void main() {
  vec3 n = normalize(v_n);
  vec3 l = normalize(u_light);
  float diff = max(dot(n, l), 0.0);
  float spec = pow(max(dot(reflect(-l, n), vec3(0.0, 0.0, 1.0)), 0.0), 24.0) * 0.25;
  vec3 base = mix(u_top, u_deep, u_ramp * smoothstep(0.0, 1.0, v_d));
  o_color = vec4(base * (0.32 + 0.72 * diff) + vec3(spec), 1.0);
}`;

export interface Camera {
  /** Turn about the vertical, radians (0: seen from the operator's side). */
  yaw: number;
  /** From straight down (0) towards the horizon, radians. */
  pitch: number;
  /** Distance from the target, mm. */
  dist: number;
  /** The point looked at, field mm (x, y). */
  tx: number;
  ty: number;
}

type Mat4 = Float32Array;

function perspective(fovY: number, aspect: number, near: number, far: number): Mat4 {
  const f = 1 / Math.tan(fovY / 2);
  const m = new Float32Array(16);
  m[0] = f / aspect;
  m[5] = f;
  m[10] = (far + near) / (near - far);
  m[11] = -1;
  m[14] = (2 * far * near) / (near - far);
  return m;
}

function multiply(a: Mat4, b: Mat4): Mat4 {
  const o = new Float32Array(16);
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[k * 4 + r] * b[c * 4 + k];
      o[c * 4 + r] = s;
    }
  return o;
}

/** The camera's basis: right, up and forward, and the eye position (field mm, z up). */
export function cameraBasis(cam: Camera, tz: number): { r: number[]; u: number[]; f: number[]; eye: number[] } {
  const sp = Math.sin(cam.pitch);
  const cp = Math.cos(cam.pitch);
  const sy = Math.sin(cam.yaw);
  const cy = Math.cos(cam.yaw);
  const d = [sp * sy, -sp * cy, cp];
  const eye = [cam.tx + d[0] * cam.dist, cam.ty + d[1] * cam.dist, tz + d[2] * cam.dist];
  const f = [-d[0], -d[1], -d[2]];
  const r = [cy, sy, 0];
  const u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
  return { r, u, f, eye };
}

function view(cam: Camera, tz: number): Mat4 {
  const { r, u, f, eye } = cameraBasis(cam, tz);
  const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  // Column-major: rows r, u, -f.
  return new Float32Array([r[0], u[0], -f[0], 0, r[1], u[1], -f[1], 0, r[2], u[2], -f[2], 0, -dot(r, eye), -dot(u, eye), dot(f, eye), 1]);
}

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const s = gl.createShader(type);
  if (!s) throw new Error("WebGL: no shader");
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(`WebGL shader: ${gl.getShaderInfoLog(s) ?? "?"}`);
  return s;
}

/** CSS colour (#rrggbb) as 0..1 RGB. */
function rgb(css: string, fallback: [number, number, number]): [number, number, number] {
  const m = /^#([0-9a-f]{6})$/i.exec(css.trim());
  if (!m) return fallback;
  const v = parseInt(m[1], 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

export class ReliefGL {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private vao: WebGLVertexArrayObject;
  private pos: WebGLBuffer;
  private depth: WebGLBuffer;
  private normal: WebGLBuffer;
  private index: WebGLBuffer;
  private count = 0;
  private gridKey = "";
  private loc: Record<string, WebGLUniformLocation | null> = {};
  private field: ReliefField | null = null;
  private exag = 1;
  /** Colour by depth (else one metal colour). */
  ramp = true;

  private canvas: HTMLCanvasElement;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const gl = canvas.getContext("webgl2", { antialias: true, alpha: false, preserveDrawingBuffer: true });
    if (!gl) throw new Error("WebGL2 is not available in this browser");
    this.gl = gl;
    const prog = gl.createProgram();
    if (!prog) throw new Error("WebGL: no program");
    gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VS));
    gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FS));
    gl.bindAttribLocation(prog, 0, "a_pos");
    gl.bindAttribLocation(prog, 1, "a_depth");
    gl.bindAttribLocation(prog, 2, "a_normal");
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(`WebGL program: ${gl.getProgramInfoLog(prog) ?? "?"}`);
    this.prog = prog;
    for (const u of ["u_mvp", "u_exag", "u_max", "u_light", "u_top", "u_deep", "u_ramp"]) this.loc[u] = gl.getUniformLocation(prog, u);
    const vao = gl.createVertexArray();
    const mk = () => {
      const b = gl.createBuffer();
      if (!b) throw new Error("WebGL: no buffer");
      return b;
    };
    if (!vao) throw new Error("WebGL: no vertex array");
    this.vao = vao;
    this.pos = mk();
    this.depth = mk();
    this.normal = mk();
    this.index = mk();
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.pos);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.depth);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 1, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.normal);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 3, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.index);
    gl.bindVertexArray(null);
  }

  /** A new field (and its normals at the current exaggeration). */
  setField(f: ReliefField, normals: Float32Array, exaggeration: number): void {
    const gl = this.gl;
    this.field = f;
    this.exag = exaggeration;
    gl.bindVertexArray(this.vao);
    const key = `${f.cols}x${f.rows}@${f.x0},${f.y0},${f.cell}`;
    if (key !== this.gridKey) {
      this.gridKey = key;
      const pos = new Float32Array(f.cols * f.rows * 2);
      for (let cy = 0; cy < f.rows; cy++)
        for (let cx = 0; cx < f.cols; cx++) {
          const o = 2 * (cy * f.cols + cx);
          pos[o] = f.x0 + cx * f.cell;
          pos[o + 1] = f.y0 + cy * f.cell;
        }
      gl.bindBuffer(gl.ARRAY_BUFFER, this.pos);
      gl.bufferData(gl.ARRAY_BUFFER, pos, gl.STATIC_DRAW);
      const idx = new Uint32Array((f.cols - 1) * (f.rows - 1) * 6);
      let k = 0;
      for (let cy = 0; cy < f.rows - 1; cy++)
        for (let cx = 0; cx < f.cols - 1; cx++) {
          const a = cy * f.cols + cx;
          const b = a + 1;
          const c = a + f.cols;
          const d = c + 1;
          idx[k++] = a;
          idx[k++] = b;
          idx[k++] = d;
          idx[k++] = a;
          idx[k++] = d;
          idx[k++] = c;
        }
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.index);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idx, gl.STATIC_DRAW);
      this.count = idx.length;
      gl.bindBuffer(gl.ARRAY_BUFFER, this.depth);
      gl.bufferData(gl.ARRAY_BUFFER, f.depth, gl.DYNAMIC_DRAW);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.normal);
      gl.bufferData(gl.ARRAY_BUFFER, normals, gl.DYNAMIC_DRAW);
    } else {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.depth);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, f.depth);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.normal);
      gl.bufferSubData(gl.ARRAY_BUFFER, 0, normals);
    }
    gl.bindVertexArray(null);
  }

  /** Draw with `cam`; `maxDepth` is the depth the colour ramp ends at (the whole job's, so scrubbing keeps colours). */
  draw(cam: Camera, maxDepth: number, background: string): void {
    const gl = this.gl;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    const [br, bg, bb] = rgb(background, [0.13, 0.14, 0.16]);
    gl.clearColor(br, bg, bb, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const f = this.field;
    if (!f || this.count === 0) return;
    gl.enable(gl.DEPTH_TEST);
    gl.useProgram(this.prog);
    const tz = (-maxDepth * this.exag) / 2;
    const proj = perspective((35 * Math.PI) / 180, w / h, cam.dist / 100, cam.dist * 10);
    gl.uniformMatrix4fv(this.loc.u_mvp, false, multiply(proj, view(cam, tz)));
    gl.uniform1f(this.loc.u_exag, this.exag);
    gl.uniform1f(this.loc.u_max, maxDepth);
    gl.uniform3f(this.loc.u_light, -0.45, 0.35, 0.82);
    gl.uniform3f(this.loc.u_top, 0.8, 0.82, 0.85);
    gl.uniform3f(this.loc.u_deep, 0.86, 0.47, 0.16);
    gl.uniform1f(this.loc.u_ramp, this.ramp ? 1 : 0);
    gl.bindVertexArray(this.vao);
    gl.drawElements(gl.TRIANGLES, this.count, gl.UNSIGNED_INT, 0);
    gl.bindVertexArray(null);
  }

  dispose(): void {
    const gl = this.gl;
    for (const b of [this.pos, this.depth, this.normal, this.index]) gl.deleteBuffer(b);
    gl.deleteVertexArray(this.vao);
    gl.deleteProgram(this.prog);
  }
}

/** The camera that shows `f` whole, tilted `pitchDeg` from straight down. */
export function fitCamera(f: ReliefField, pitchDeg = 55, yawDeg = 0): Camera {
  const w = f.cols * f.cell;
  const h = f.rows * f.cell;
  return {
    yaw: (yawDeg * Math.PI) / 180,
    pitch: (pitchDeg * Math.PI) / 180,
    dist: Math.max(w, h) * 1.9,
    tx: f.x0 + (w - f.cell) / 2,
    ty: f.y0 + (h - f.cell) / 2,
  };
}
