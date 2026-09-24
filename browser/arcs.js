// Relationships as animated arcs lifted off the globe (zega#74).
//
// A MapLibre custom layer. Each arc is one instance: the GPU walks the great
// circle from a per-arc orthonormal basis (u, v, angular length), lifts the
// route by a sine bump and extrudes a screen-space ribbon, so 5,000 arcs are
// one draw call and the CPU does no per-frame work. On the sphere, the globe
// itself is drawn depth-only first; arcs behind the planet fail the depth
// test, and an arc rising over the limb shows where it clears the surface.
// Afterwards the sphere is drawn again to put the depth buffer back, so the
// marker layer above is unaffected.
//
// MapLibre 6.11.1 contract (vendor/maplibre-gl/maplibre-gl.mjs,
// `getProjectionDataForCustomLayer`): while the globe shows,
// `defaultProjectionData.mainMatrix` maps unit-sphere positions to clip space
// and `fallbackMatrix` maps mercator [0,1]² plus altitude in world widths;
// `projectionTransition` is 1 on the sphere and 0 on the flat map, blending
// between zoom 11 and 12. The sphere convention is MapLibre's
// `projectToSphere`: (sin lon · cos lat, sin lat, cos lon · cos lat).
// `clippingPlane.xyz` is the camera direction from the globe's centre.
//
// The flat positions are not computed in the shader: GLSL gives no precision
// for atan/asin/tan, and at zoom 12 a 1e-5 rad error is 60 m, several pixels.
// They are computed per point in float64 here and read from a float texture.

const SEGMENTS = 64;
const POINTS = SEGMENTS + 1;
const LINE_WIDTH = 1.6; // CSS px
const PULSE_RADIUS = 3.2; // CSS px
const DASH = [6, 5]; // on, off in CSS px
const DASH_SPEED = 20; // CSS px per second, towards the target
const PULSE_SPEED = 0.25; // arcs per second
const PICK_HALF_WIDTH = 6; // CSS px: a comfortable click target
const MAX_LAT = 85.051129;
const FLOATS_PER_ARC = 10; // u(3) v(3) omega phase id x0

// The lift of a route, in globe radii: the approved preview's
// `lift·(0.25 + 0.75·d/π)`, capped at twice the angular length so a short hop
// arches rather than rising hundreds of kilometres above the camera.
export function arcHeight(lift, omega) {
  return lift * Math.min(0.25 + 0.75 * omega / Math.PI, 2 * omega);
}

export function sphere(lon, lat) {
  const phi = Math.max(-89.99, Math.min(89.99, lat)) * Math.PI / 180, lambda = lon * Math.PI / 180;
  const c = Math.cos(phi);
  return [Math.sin(lambda) * c, Math.sin(phi), Math.cos(lambda) * c];
}

// The great circle from `from` to `to` as an orthonormal basis: the route is
// cos(s·Ω)·u + sin(s·Ω)·v for s in [0, 1]. Computed once in float64, so an
// antipodal pair takes the same (northward) route every frame instead of a
// direction that depends on rounding.
export function greatCircle(from, to) {
  const u = sphere(from[0], from[1]), b = sphere(to[0], to[1]);
  const dot = Math.max(-1, Math.min(1, u[0] * b[0] + u[1] * b[1] + u[2] * b[2]));
  const cross = [u[1] * b[2] - u[2] * b[1], u[2] * b[0] - u[0] * b[2], u[0] * b[1] - u[1] * b[0]];
  const omega = Math.atan2(Math.hypot(...cross), dot);
  let v = b.map((x, i) => x - dot * u[i]);
  let len = Math.hypot(...v);
  if (len < 1e-9) {
    const ref = Math.abs(u[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0];
    const d = ref[0] * u[0] + ref[1] * u[1] + ref[2] * u[2];
    v = ref.map((x, i) => x - d * u[i]);
    len = Math.hypot(...v);
  }
  return { u, v: v.map((x) => x / len), omega };
}

const pointAt = ({ u, v, omega }, s) => u.map((x, i) => Math.cos(s * omega) * x + Math.sin(s * omega) * v[i]);

// Mercator x (unwrapped to stay next to `previous`), y and latitude of a unit-sphere point.
function mercator(p, previous) {
  const lat = Math.max(-MAX_LAT, Math.min(MAX_LAT, Math.asin(Math.max(-1, Math.min(1, p[1]))) * 180 / Math.PI)) * Math.PI / 180;
  let x = Math.atan2(p[0], p[2]) / (2 * Math.PI) + 0.5;
  if (previous != null) x += Math.round(previous - x);
  return [x, 0.5 - Math.log(Math.tan(Math.PI / 4 + lat / 2)) / (2 * Math.PI), lat];
}

const COMMON = `
const float PI = 3.141592653589793;
uniform mat4 u_globe;
uniform mat4 u_flat;
uniform float u_transition;
uniform float u_center;
uniform float u_w_center;
uniform vec3 u_eye;
uniform float u_lift;
// Clip-space position of a unit-sphere point p lifted by h radii, whose
// mercator x, y and latitude are merc and whose arc starts at mercator x0,
// following MapLibre's own sphere-to-mercator blend. Depth on the sphere is
// the distance along the camera direction: nearer is smaller, never clipped.
// The flat position is the world copy nearest the map's centre. A point far
// enough round the globe to reach the camera plane keeps its flat position
// alone, so a long route cannot stray across the view mid-blend.
vec4 project(vec3 p, float h, vec3 merc, float x0) {
  vec3 lifted = p * (1.0 + h);
  vec4 globe = u_globe * vec4(lifted, 1.0);
  globe.z = -dot(lifted, u_eye) * 0.5 * globe.w;
  if (u_transition > 0.999) return globe;
  float shift = floor(u_center - x0 + 0.5);
  vec4 plane = u_flat * vec4(merc.x + shift, merc.y, h / (2.0 * PI * cos(merc.z)), 1.0);
  float t = u_transition * smoothstep(0.0, 0.5, globe.w / u_w_center);
  return mix(plane, globe, t);
}
float height(float omega) { return u_lift * min(0.25 + 0.75 * omega / PI, 2.0 * omega); }
`;

const ARC_VERTEX = `#version 300 es
precision highp float;
precision highp sampler2D;
${COMMON}
layout(location=0) in vec2 a_t;
layout(location=1) in vec3 a_u;
layout(location=2) in vec3 a_v;
layout(location=3) in vec4 a_arc;
uniform sampler2D u_merc;
uniform int u_per_row;
uniform vec2 u_viewport;
uniform float u_extrude;
uniform float u_px_per_rad;
out float v_side;
out float v_along;
flat out float v_id;
vec3 merc(int i) {
  return texelFetch(u_merc, ivec2((gl_InstanceID % u_per_row) * ${POINTS} + i, gl_InstanceID / u_per_row), 0).xyz;
}
vec4 at(int i, float h) {
  float s = float(i) / ${SEGMENTS}.0;
  float a = s * a_arc.x;
  return project(cos(a) * a_u + sin(a) * a_v, h * sin(PI * s), merc(i), a_arc.w);
}
vec2 screen(vec4 c) { return c.xy / c.w * u_viewport * 0.5; }
void main() {
  int i = int(a_t.x);
  float h = height(a_arc.x);
  vec4 c = at(i, h);
  vec4 c0 = at(max(i - 1, 0), h);
  vec4 c1 = at(min(i + 1, ${SEGMENTS}), h);
  vec2 p = screen(c);
  vec2 p0 = c0.w > 0.0 ? screen(c0) : p;
  vec2 p1 = c1.w > 0.0 ? screen(c1) : p;
  vec2 dir = p1 - p0;
  float len = length(dir);
  dir = len > 1e-3 ? dir / len : vec2(1.0, 0.0);
  c.xy += vec2(-dir.y, dir.x) * a_t.y * u_extrude / (u_viewport * 0.5) * c.w;
  gl_Position = c;
  v_side = a_t.y * u_extrude;
  v_along = a_t.x / ${SEGMENTS}.0 * a_arc.x * u_px_per_rad;
  v_id = a_arc.z;
}`;

const ARC_FRAGMENT = `#version 300 es
precision highp float;
in float v_side;
in float v_along;
flat in float v_id;
uniform vec4 u_color;
uniform float u_half_width;
uniform float u_animate;
uniform float u_time;
uniform float u_pick;
uniform vec2 u_dash;
uniform float u_speed;
out vec4 fragColor;
void main() {
  float d = abs(v_side);
  if (u_pick > 0.5) {
    if (d > u_half_width) discard;
    fragColor = vec4(mod(v_id, 256.0), mod(floor(v_id / 256.0), 256.0), floor(v_id / 65536.0), 255.0) / 255.0;
    return;
  }
  float edge = 1.0 - smoothstep(u_half_width - 0.5, u_half_width + 0.5, d);
  float dash = 1.0;
  if (u_animate > 0.5) {
    float phase = mod(v_along - u_time * u_speed, u_dash.x + u_dash.y);
    dash = smoothstep(0.0, 0.75, phase) * (1.0 - smoothstep(u_dash.x - 0.75, u_dash.x, phase));
  }
  float alpha = edge * dash * u_color.a;
  fragColor = vec4(u_color.rgb * alpha, alpha);
}`;

const PULSE_VERTEX = `#version 300 es
precision highp float;
precision highp sampler2D;
${COMMON}
layout(location=1) in vec3 a_u;
layout(location=2) in vec3 a_v;
layout(location=3) in vec4 a_arc;
uniform sampler2D u_merc;
uniform int u_per_row;
uniform float u_time;
uniform float u_point_size;
vec3 merc(int i) {
  return texelFetch(u_merc, ivec2((gl_VertexID % u_per_row) * ${POINTS} + i, gl_VertexID / u_per_row), 0).xyz;
}
void main() {
  float s = fract(u_time * ${PULSE_SPEED} + a_arc.y);
  float a = s * a_arc.x;
  float k = s * ${SEGMENTS}.0;
  int i = int(floor(k));
  vec3 m = mix(merc(i), merc(min(i + 1, ${SEGMENTS})), k - float(i));
  gl_Position = project(cos(a) * a_u + sin(a) * a_v, height(a_arc.x) * sin(PI * s), m, a_arc.w);
  gl_PointSize = u_point_size;
}`;

const PULSE_FRAGMENT = `#version 300 es
precision highp float;
uniform vec4 u_color;
uniform float u_point_size;
uniform float u_radius;
out vec4 fragColor;
void main() {
  float d = length(gl_PointCoord - 0.5) * u_point_size;
  float alpha = (1.0 - smoothstep(u_radius - 0.75, u_radius + 0.75, d)) * u_color.a;
  fragColor = vec4(u_color.rgb * alpha, alpha);
}`;

// The sphere, depth only, drawn on the pure globe: the same depth as the arcs.
const SURFACE_VERTEX = `#version 300 es
precision highp float;
uniform mat4 u_globe;
uniform vec3 u_eye;
uniform float u_reset;
layout(location=0) in vec3 a_pos;
void main() {
  vec4 c = u_globe * vec4(a_pos, 1.0);
  c.z = u_reset > 0.5 ? c.w : -dot(a_pos, u_eye) * 0.5 * c.w;
  gl_Position = c;
}`;

const SURFACE_FRAGMENT = `#version 300 es
precision highp float;
out vec4 fragColor;
void main() { fragColor = vec4(0.0); }`;

function compile(gl, vertex, fragment) {
  const program = gl.createProgram();
  const shaders = [];
  try {
    for (const [type, source] of [[gl.VERTEX_SHADER, vertex], [gl.FRAGMENT_SHADER, fragment]]) {
      const shader = gl.createShader(type);
      shaders.push({ shader, attached: false });
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(`arc shader: ${gl.getShaderInfoLog(shader)}`);
      gl.attachShader(program, shader);
      shaders[shaders.length - 1].attached = true;
    }
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(`arc program: ${gl.getProgramInfoLog(program)}`);
  } finally {
    // Once linked, the program keeps its own copy: the shader objects are
    // freed here rather than left attached, where a delete would wait for
    // the program's and every view leaked six of them (zega#83).
    for (const { shader, attached } of shaders) {
      if (attached) gl.detachShader(program, shader);
      gl.deleteShader(shader);
    }
  }
  const uniforms = {};
  for (let i = 0; i < gl.getProgramParameter(program, gl.ACTIVE_UNIFORMS); i++) {
    const name = gl.getActiveUniform(program, i).name;
    uniforms[name] = gl.getUniformLocation(program, name);
  }
  return { program, uniforms };
}

// The unit sphere as a lon/lat grid.
function surfaceMesh() {
  const LON = 128, LAT = 64;
  const vertices = [], indices = [];
  for (let j = 0; j <= LAT; j++) {
    for (let i = 0; i <= LON; i++) {
      vertices.push(...sphere(-180 + 360 * i / LON, -90 + 180 * j / LAT));
      if (i < LON && j < LAT) {
        const a = j * (LON + 1) + i, b = a + LON + 1;
        indices.push(a, b, a + 1, a + 1, b, b + 1);
      }
    }
  }
  return { vertices: new Float32Array(vertices), indices: new Uint16Array(indices), count: indices.length };
}

function parseColor(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, 1];
}

function multiply(m, v) {
  return [0, 1, 2, 3].map((i) => m[i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2] + m[12 + i] * v[3]);
}

export class ArcLayer {
  constructor({ color, lift = 0.18, animate = true }) {
    this.id = 'zega-arcs';
    this.type = 'custom';
    this.renderingMode = '3d';
    this.color = parseColor(color);
    this.lift = lift;
    this.animate = animate;
    this.records = [];
    this.geometry = [];
    this.instances = new Float32Array(0);
    this.points = new Float32Array(0);
    this.frames = 0;
    this.picks = 0;
    this.frame = null;
    this.pending = null;
    this.gl = null;
  }

  get count() { return this.records.length; }

  /** `records`: `{ from: [lon, lat], to: [lon, lat], rel }` per arc. */
  setArcs(records) {
    this.records = records;
    this.geometry = records.map(({ from, to }) => greatCircle(from, to));
    this.instances = new Float32Array(records.length * FLOATS_PER_ARC);
    this.points = new Float32Array(records.length * POINTS * 4);
    this.geometry.forEach((arc, i) => {
      let previous = null;
      for (let j = 0; j < POINTS; j++) {
        const m = mercator(pointAt(arc, j / SEGMENTS), previous);
        if (j === 0) arc.x0 = m[0];
        previous = m[0];
        this.points.set(m, (i * POINTS + j) * 4);
      }
      this.instances.set([...arc.u, ...arc.v, arc.omega, (i * 0.618033988749895) % 1, i + 1, arc.x0], i * FLOATS_PER_ARC);
    });
    if (this.gl) this.upload();
    this.map?.triggerRepaint();
  }

  setSettings({ lift = this.lift, animate = this.animate }) {
    this.lift = Math.max(0, Math.min(0.5, lift));
    this.animate = animate;
    this.map?.triggerRepaint();
  }

  setColor(hex) {
    this.color = parseColor(hex);
    this.map?.triggerRepaint();
  }

  onAdd(map, gl) {
    this.map = map;
    this.gl = gl;
    this.arcs = compile(gl, ARC_VERTEX, ARC_FRAGMENT);
    this.pulses = compile(gl, PULSE_VERTEX, PULSE_FRAGMENT);
    this.surface = compile(gl, SURFACE_VERTEX, SURFACE_FRAGMENT);
    const template = new Float32Array(POINTS * 4);
    for (let i = 0; i < POINTS; i++) template.set([i, 1, i, -1], i * 4);
    this.templateBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.templateBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, template, gl.STATIC_DRAW);
    this.instanceBuffer = gl.createBuffer();
    this.texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    for (const [key, value] of [[gl.TEXTURE_MIN_FILTER, gl.NEAREST], [gl.TEXTURE_MAG_FILTER, gl.NEAREST], [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, key, value);
    this.perRow = Math.max(1, Math.floor(gl.getParameter(gl.MAX_TEXTURE_SIZE) / POINTS));
    const mesh = surfaceMesh();
    this.surfaceCount = mesh.count;
    this.surfaceBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.surfaceBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, mesh.vertices, gl.STATIC_DRAW);
    this.surfaceIndices = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.surfaceIndices);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, mesh.indices, gl.STATIC_DRAW);
    this.upload();
  }

  onRemove(map, gl) {
    for (const { program } of [this.arcs, this.pulses, this.surface]) gl.deleteProgram(program);
    for (const buffer of [this.templateBuffer, this.instanceBuffer, this.surfaceBuffer, this.surfaceIndices]) gl.deleteBuffer(buffer);
    gl.deleteTexture(this.texture);
    this.dropPickTarget(gl);
    this.pending?.resolve(null);
    this.pending = null;
    this.gl = null;
    this.map = null;
  }

  upload() {
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instanceBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.instances, gl.STATIC_DRAW);
    if (!this.count) return;
    const rows = Math.ceil(this.count / this.perRow), width = this.perRow * POINTS;
    const data = new Float32Array(width * rows * 4);
    data.set(this.points);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA32F, width, rows, 0, gl.RGBA, gl.FLOAT, data);
  }

  /** The relationship under a canvas point (CSS px), read back from an id render. */
  pick(x, y) {
    if (!this.gl || !this.count) return Promise.resolve(null);
    this.pending?.resolve(null);
    return new Promise((resolve) => {
      this.pending = { x, y, resolve };
      this.map.triggerRepaint();
    });
  }

  /** Where arc `index` at parameter `s` lands on the canvas (CSS px) in the last frame, or null when behind the camera. */
  screen(index, s) {
    const f = this.frame, g = this.geometry[index];
    if (!f || !g) return null;
    const p = pointAt(g, s);
    const h = arcHeight(this.lift, g.omega) * Math.sin(Math.PI * s);
    const lifted = p.map((x) => x * (1 + h));
    let c = multiply(f.globe, [...lifted, 1]);
    c[2] = -(lifted[0] * f.eye[0] + lifted[1] * f.eye[1] + lifted[2] * f.eye[2]) * 0.5 * c[3];
    if (f.t <= 0.999) {
      const [x, y, lat] = mercator(p, g.x0);
      const plane = multiply(f.flat, [x + Math.round(f.center - g.x0), y, h / (2 * Math.PI * Math.cos(lat)), 1]);
      const k = Math.max(0, Math.min(1, c[3] / f.wCenter / 0.5));
      const t = f.t * k * k * (3 - 2 * k);
      c = c.map((v, i) => plane[i] + (v - plane[i]) * t);
    }
    if (c[3] <= 0) return null;
    return { x: (c[0] / c[3] + 1) / 2 * f.width / f.dpr, y: (1 - c[1] / c[3]) / 2 * f.height / f.dpr };
  }

  render(gl, args) {
    const data = args.defaultProjectionData;
    const globe = args.shaderData.variantName === 'globe';
    const t = globe ? data.projectionTransition : 0;
    const plane = globe ? data.clippingPlane : [0, 0, 1, 0];
    const norm = Math.hypot(plane[0], plane[1], plane[2]) || 1;
    const dpr = this.map.getPixelRatio();
    const center = this.map.getCenter();
    this.frame = {
      globe: data.mainMatrix, flat: globe ? data.fallbackMatrix : data.mainMatrix, t,
      eye: [plane[0] / norm, plane[1] / norm, plane[2] / norm],
      width: gl.drawingBufferWidth, height: gl.drawingBufferHeight, dpr,
      center: center.lng / 360 + 0.5,
      wCenter: multiply(data.mainMatrix, [...sphere(center.lng, center.lat), 1])[3] || 1,
      // MapLibre's globe radius in pixels: the 512 px world at this zoom,
      // scaled so the sphere matches the flat map at the centre latitude.
      pxPerRad: 512 * 2 ** this.map.getZoom() * dpr / (2 * Math.PI) / Math.cos(center.lat * Math.PI / 180),
      time: performance.now() / 1000,
    };
    if (this.pending) this.doPick(gl);
    if (!this.count) return;
    gl.enable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.STENCIL_TEST);
    gl.depthRange(0, 1);
    this.drawSurface(gl, false);
    gl.colorMask(true, true, true, true);
    gl.depthMask(false);
    gl.depthFunc(gl.LEQUAL);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    this.drawArcs(gl, false);
    if (this.animate) this.drawPulses(gl);
    this.drawSurface(gl, true);
    gl.colorMask(true, true, true, true);
    this.frames++;
    if (this.animate) this.map.triggerRepaint();
  }

  common(gl, { uniforms }) {
    const f = this.frame;
    gl.uniformMatrix4fv(uniforms.u_globe, false, Float32Array.from(f.globe));
    gl.uniformMatrix4fv(uniforms.u_flat, false, Float32Array.from(f.flat));
    gl.uniform1f(uniforms.u_transition, f.t);
    gl.uniform1f(uniforms.u_center, f.center);
    gl.uniform1f(uniforms.u_w_center, f.wCenter);
    gl.uniform3fv(uniforms.u_eye, f.eye);
    gl.uniform1f(uniforms.u_lift, this.lift);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.uniform1i(uniforms.u_merc, 0);
    gl.uniform1i(uniforms.u_per_row, this.perRow);
  }

  // The sphere, depth only, while the globe is round: on the flat map and in
  // the blend the view is a patch of the surface with nothing behind it.
  // `reset` writes the far depth back so the layers MapLibre draws next are
  // not hidden behind the sphere.
  drawSurface(gl, reset) {
    if (this.frame.t <= 0.999) return;
    gl.useProgram(this.surface.program);
    gl.colorMask(false, false, false, false);
    gl.depthMask(true);
    gl.depthFunc(gl.ALWAYS);
    gl.uniformMatrix4fv(this.surface.uniforms.u_globe, false, Float32Array.from(this.frame.globe));
    gl.uniform3fv(this.surface.uniforms.u_eye, this.frame.eye);
    gl.uniform1f(this.surface.uniforms.u_reset, reset ? 1 : 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.surfaceBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 12, 0);
    gl.vertexAttribDivisor(0, 0);
    for (const location of [1, 2, 3]) gl.disableVertexAttribArray(location);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.surfaceIndices);
    gl.drawElements(gl.TRIANGLES, this.surfaceCount, gl.UNSIGNED_SHORT, 0);
  }

  bindInstances(gl, instanced) {
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instanceBuffer);
    const stride = FLOATS_PER_ARC * 4;
    for (const [location, size, offset] of [[1, 3, 0], [2, 3, 12], [3, 4, 24]]) {
      gl.enableVertexAttribArray(location);
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, stride, offset);
      gl.vertexAttribDivisor(location, instanced ? 1 : 0);
    }
  }

  drawArcs(gl, pick) {
    const { program, uniforms } = this.arcs;
    const f = this.frame;
    gl.useProgram(program);
    this.common(gl, this.arcs);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.templateBuffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 8, 0);
    gl.vertexAttribDivisor(0, 0);
    this.bindInstances(gl, true);
    const halfWidth = (pick ? PICK_HALF_WIDTH : LINE_WIDTH / 2) * f.dpr;
    gl.uniform2f(uniforms.u_viewport, f.width, f.height);
    gl.uniform1f(uniforms.u_extrude, halfWidth + 1);
    gl.uniform1f(uniforms.u_half_width, halfWidth);
    gl.uniform1f(uniforms.u_px_per_rad, f.pxPerRad);
    gl.uniform4fv(uniforms.u_color, this.color);
    gl.uniform1f(uniforms.u_animate, this.animate && !pick ? 1 : 0);
    gl.uniform1f(uniforms.u_time, f.time);
    gl.uniform1f(uniforms.u_pick, pick ? 1 : 0);
    gl.uniform2f(uniforms.u_dash, DASH[0] * f.dpr, DASH[1] * f.dpr);
    gl.uniform1f(uniforms.u_speed, DASH_SPEED * f.dpr);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, POINTS * 2, this.count);
    for (const location of [1, 2, 3]) gl.vertexAttribDivisor(location, 0);
  }

  drawPulses(gl) {
    const { program, uniforms } = this.pulses;
    const f = this.frame;
    gl.useProgram(program);
    this.common(gl, this.pulses);
    gl.disableVertexAttribArray(0);
    this.bindInstances(gl, false);
    gl.uniform1f(uniforms.u_time, f.time);
    gl.uniform1f(uniforms.u_point_size, (PULSE_RADIUS * 2 + 2) * f.dpr);
    gl.uniform1f(uniforms.u_radius, PULSE_RADIUS * f.dpr);
    gl.uniform4fv(uniforms.u_color, this.color);
    gl.drawArrays(gl.POINTS, 0, this.count);
  }

  ensurePickTarget(gl, width, height) {
    if (this.pickTarget && this.pickTarget.width === width && this.pickTarget.height === height) return;
    this.dropPickTarget(gl);
    const framebuffer = gl.createFramebuffer();
    const color = gl.createRenderbuffer();
    gl.bindRenderbuffer(gl.RENDERBUFFER, color);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.RGBA8, width, height);
    const depth = gl.createRenderbuffer();
    gl.bindRenderbuffer(gl.RENDERBUFFER, depth);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH_COMPONENT24, width, height);
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, color);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, depth);
    this.pickTarget = { framebuffer, color, depth, width, height };
  }

  dropPickTarget(gl) {
    if (!this.pickTarget) return;
    gl.deleteFramebuffer(this.pickTarget.framebuffer);
    gl.deleteRenderbuffer(this.pickTarget.color);
    gl.deleteRenderbuffer(this.pickTarget.depth);
    this.pickTarget = null;
  }

  // Draw the sphere's depth and the arcs as ids into an offscreen target and
  // read the pixels around the point: what the user sees is what they hit.
  doPick(gl) {
    const { x, y, resolve } = this.pending;
    this.pending = null;
    this.picks++;
    const f = this.frame;
    if (!this.count) return resolve(null);
    const previous = gl.getParameter(gl.FRAMEBUFFER_BINDING);
    this.ensurePickTarget(gl, f.width, f.height);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.pickTarget.framebuffer);
    gl.viewport(0, 0, f.width, f.height);
    gl.disable(gl.SCISSOR_TEST);
    gl.disable(gl.BLEND);
    gl.disable(gl.CULL_FACE);
    gl.enable(gl.DEPTH_TEST);
    gl.depthRange(0, 1);
    gl.colorMask(true, true, true, true);
    gl.depthMask(true);
    gl.clearColor(0, 0, 0, 0);
    gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    this.drawSurface(gl, false);
    gl.colorMask(true, true, true, true);
    gl.depthMask(false);
    gl.depthFunc(gl.LEQUAL);
    this.drawArcs(gl, true);
    const r = Math.round(4 * f.dpr), side = 2 * r + 1;
    const cx = Math.round(x * f.dpr), cy = Math.round(f.height - y * f.dpr);
    const pixels = new Uint8Array(side * side * 4);
    gl.readPixels(cx - r, cy - r, side, side, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    gl.bindFramebuffer(gl.FRAMEBUFFER, previous);
    gl.viewport(0, 0, f.width, f.height);
    let best = null, bestDistance = Infinity;
    for (let j = 0; j < side; j++) {
      for (let i = 0; i < side; i++) {
        const o = (j * side + i) * 4;
        const id = pixels[o] + pixels[o + 1] * 256 + pixels[o + 2] * 65536;
        const distance = Math.hypot(i - r, j - r);
        if (id && distance < bestDistance) { best = id; bestDistance = distance; }
      }
    }
    resolve(best ? this.records[best - 1]?.rel ?? null : null);
  }
}
