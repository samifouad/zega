// Build the Monaco sample for the path-on-map graph component (APS 21):
// the Circuit de Monaco racing line from OpenStreetMap, and a small basemap
// extract that covers it (the shared cities.pmtiles has no Monaco).
//
// Build-only tool: never uploads or reads credentials.
// Usage: node scripts/circuit-sample.mjs /path/to/pmtiles
//
// Geometry: OSM relation 148194 (type=circuit), its unroled member ways in
// relation order, stitched end to end and started at its `start-finish` node.
// `speed` is MODELLED, not telemetry: a point-mass lap over the line's
// curvature (lateral, braking and traction limits below). It is there so the
// component has a real per-point value to colour by; `dist` is exact.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
const [pmtiles] = process.argv.slice(2);
if (!pmtiles) throw new Error('Usage: node scripts/circuit-sample.mjs /path/to/pmtiles');

const RELATION = 148194;
const STEP = 10; // metres between resampled track points
// The basemap extract: the circuit's box with a margin, zooms 13-15
// (the component frames the lap at about zoom 14; 15 carries building heights).
const BBOX = [7.405, 43.723, 7.440, 43.748];
const BUILD = '20260923';

await mkdir('.tmp', { recursive: true });
const cache = `.tmp/osm-relation-${RELATION}.json`;
let raw;
try { raw = JSON.parse(await readFile(cache, 'utf8')); } catch {
  const response = await fetch('https://overpass-api.de/api/interpreter', {
    method: 'POST',
    headers: { 'User-Agent': 'zega-explorer-sample-builder/1.0 (github.com/zegadb/zega)', 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ data: `[out:json][timeout:50];relation(${RELATION});out geom;` }),
  });
  if (!response.ok) throw new Error(`Overpass: HTTP ${response.status}`);
  raw = await response.json();
  await writeFile(cache, JSON.stringify(raw));
}
const osmTime = raw.osm3s.timestamp_osm_base;
const relation = raw.elements[0];
const lengthTag = Number(relation.tags.length);

// Stitch the unroled ways into one ordered line. Relation order is not race
// order and some ways are mapped backwards, so chain greedily from the way
// through the start-finish node, in its mapped (oneway, race) direction, to
// whichever unused way has an end nearest the chain's end. OSM leaves small
// gaps where the racing line leaves a street (a few metres); a gap over
// MAX_GAP is an error. Ways left over (the pit exit shares a street) are unused.
const MAX_GAP = 30;
const ways = relation.members.filter((m) => m.type === 'way' && m.role === '').map((m) => ({ id: m.ref, pts: m.geometry.map((p) => [p.lon, p.lat]) }));
const startNode = relation.members.find((m) => m.role === 'start-finish');
const metres = (a, b) => Math.hypot((a[0] - b[0]) * 111_319.49 * Math.cos(a[1] * Math.PI / 180), (a[1] - b[1]) * 111_132.954);
const toSeg = (p, a, b) => { // metres from p to segment ab (equirectangular)
  const k = Math.cos(p[1] * Math.PI / 180), ax = (a[0] - p[0]) * k, ay = a[1] - p[1], bx = (b[0] - p[0]) * k, by = b[1] - p[1];
  const dx = bx - ax, dy = by - ay, t = Math.max(0, Math.min(1, -(ax * dx + ay * dy) / (dx * dx + dy * dy || 1)));
  return Math.hypot(ax + t * dx, ay + t * dy) * 111_132.954;
};
const sf = [startNode.lon, startNode.lat];
const first = ways.reduce((best, way) => {
  const d = Math.min(...way.pts.slice(1).map((b, i) => toSeg(sf, way.pts[i], b)));
  return d < best.d ? { d, way } : best;
}, { d: Infinity }).way;
const unused = new Set(ways.filter((w) => w !== first));
let line = [...first.pts];
for (;;) {
  const end = line.at(-1);
  if (line.length > first.pts.length && metres(end, line[0]) < MAX_GAP && ![...unused].some((w) => Math.min(metres(w.pts[0], end), metres(w.pts.at(-1), end)) < metres(end, line[0]))) break;
  let pick = null;
  for (const way of unused) for (const reversed of [false, true]) {
    const pts = reversed ? [...way.pts].reverse() : way.pts;
    const d = metres(pts[0], end);
    if (!pick || d < pick.d) pick = { d, way, pts };
  }
  if (!pick || pick.d > MAX_GAP) throw new Error(`Circuit has a ${pick?.d.toFixed(0)} m gap after ${line.length} points`);
  unused.delete(pick.way);
  line.push(...(pick.d < 0.01 ? pick.pts.slice(1) : pick.pts));
}
if (metres(line[0], line.at(-1)) < 0.01) line.pop();
console.log(`stitched ${ways.length - unused.size} ways; unused: ${[...unused].map((w) => w.id).join(', ') || 'none'}`);

// Metres on a local tangent plane (the circuit spans 1.5 km).
const lat0 = 43.735 * Math.PI / 180;
const M_LAT = 111_132.954, M_LON = 111_319.49 * Math.cos(lat0);
const xy = ([lon, lat]) => [lon * M_LON, lat * M_LAT];
const lonlat = ([x, y]) => [x / M_LON, y / M_LAT];

// Start at the start-finish node, projected onto the line.
const s = xy([startNode.lon, startNode.lat]);
let best = { d: Infinity };
for (let i = 0; i < line.length; i++) {
  const a = xy(line[i]), b = xy(line[(i + 1) % line.length]);
  const dx = b[0] - a[0], dy = b[1] - a[1];
  const t = Math.max(0, Math.min(1, ((s[0] - a[0]) * dx + (s[1] - a[1]) * dy) / (dx * dx + dy * dy)));
  const p = [a[0] + t * dx, a[1] + t * dy];
  const d = Math.hypot(p[0] - s[0], p[1] - s[1]);
  if (d < best.d) best = { d, i, p };
}
const ring = [best.p, ...[...line.slice(best.i + 1), ...line.slice(0, best.i + 1)].map(xy)];

// Resample every STEP metres along the closed ring.
const segs = ring.map((a, i) => { const b = ring[(i + 1) % ring.length]; return { a, b, len: Math.hypot(b[0] - a[0], b[1] - a[1]) }; });
const total = segs.reduce((n, seg) => n + seg.len, 0);
const points = [];
let seg = 0, before = 0;
for (let d = 0; d < total - STEP / 2; d += STEP) {
  while (before + segs[seg].len < d || segs[seg].len === 0) before += segs[seg++].len;
  const t = (d - before) / segs[seg].len;
  points.push({ d, p: [segs[seg].a[0] + t * (segs[seg].b[0] - segs[seg].a[0]), segs[seg].a[1] + t * (segs[seg].b[1] - segs[seg].a[1])] });
}

// Modelled speed: curvature radius over ±2 points (±20 m), a lateral limit,
// then forward (traction) and backward (braking) passes around the lap.
const n = points.length;
const LATERAL = 3.5 * 9.81, BRAKE = 4.5 * 9.81, TRACTION = 1.2 * 9.81, TOP = 290 / 3.6;
const radius = (i) => {
  const [a, b, c] = [points[(i - 2 + n) % n].p, points[i].p, points[(i + 2) % n].p];
  const ab = Math.hypot(b[0] - a[0], b[1] - a[1]), bc = Math.hypot(c[0] - b[0], c[1] - b[1]), ca = Math.hypot(a[0] - c[0], a[1] - c[1]);
  const area2 = Math.abs((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]));
  return area2 < 1e-6 ? Infinity : (ab * bc * ca) / (2 * area2);
};
const v = points.map((_, i) => Math.min(TOP, Math.sqrt(LATERAL * radius(i))));
for (let lap = 0; lap < 2; lap++) {
  for (let k = 1; k <= n; k++) { const i = k % n, h = (k - 1) % n; v[i] = Math.min(v[i], Math.sqrt(v[h] ** 2 + 2 * TRACTION * STEP)); }
  for (let k = n - 1; k >= 0; k--) { const i = k, j = (k + 1) % n; v[i] = Math.min(v[i], Math.sqrt(v[j] ** 2 + 2 * BRAKE * STEP)); }
}

const rows = points.map(({ d, p }, i) => {
  const [lon, lat] = lonlat(p);
  return `${i},${lat.toFixed(7)},${lon.toFixed(7)},${Math.round(d)},${(v[i] * 3.6).toFixed(1)}`;
});
await writeFile('samples/monaco-track.csv', ['seq,lat,lon,dist,speed', ...rows].join('\n') + '\n');

// The basemap extract.
const archive = 'data/monaco.pmtiles';
execFileSync(pmtiles, ['extract', `https://build.protomaps.com/${BUILD}.pmtiles`, archive, `--bbox=${BBOX.join(',')}`, '--minzoom=13', '--maxzoom=15'], { stdio: 'inherit' });
execFileSync(pmtiles, ['verify', archive], { stdio: 'inherit' });
const sha = createHash('sha256').update(await readFile(archive)).digest('hex');

const zql = `// Monaco: the Circuit de Monaco racing line, © OpenStreetMap contributors (ODbL 1.0),
// https://www.openstreetmap.org/copyright — relation ${RELATION}, OSM data as of ${osmTime}.
// Generated by browser/scripts/circuit-sample.mjs: ${n} points every ${STEP} m from the
// start-finish line. \`dist\` is metres along the lap. \`speed\` is MODELLED from the
// line's curvature (km/h), not telemetry.
// Basemap: data/monaco.pmtiles, SHA-256 ${sha}
schema {
  type Circuit {
    name: String
    country: String<iso2>
    length: Int
    points: ON <- TrackPoint[]
  }

  type TrackPoint {
    seq: Int
    at: Point from (lat, lon)
    dist: Int
    speed: Float
    circuit: ON -> Circuit
  }

  // The explorer's basemap has no Monaco: the path-on-map graph component
  // (browser/components/) draws this sample, on its own extract.
  display {
    table : Default
    graph
  }
}

unique {
  Circuit { name }
  TrackPoint { seq }
}

mutation {
  Circuit(name: "Circuit de Monaco" && country: "MC" && length: ${lengthTag}) { name }
}

mutation csv ["./samples/monaco-track.csv"] {
  TrackPoint(seq: $seq && dist: $dist && speed: $speed) { seq }
}

mutation csv ["./samples/monaco-track.csv"] {
  TrackPoint(seq: $seq) {
    circuit -> link Circuit(name: "Circuit de Monaco")
  }
}
`;
await writeFile('samples/monaco.zql', zql);
const speeds = v.map((x) => x * 3.6);
console.log(`${n} points; lap ${total.toFixed(0)} m (OSM length tag ${lengthTag} m); speed ${Math.min(...speeds).toFixed(0)}-${Math.max(...speeds).toFixed(0)} km/h`);
console.log(`${archive}: ${(await stat(archive)).size} bytes, sha256 ${sha}`);
