// Rebuild data/countries-110m.geojson from Natural Earth's 1:110m Admin 0
// countries (public domain), pinned to the v5.1.2 release:
//   https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_admin_0_countries.geojson
// Usage: node scripts/prepare-countries.mjs /path/to/ne_110m_admin_0_countries.geojson
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const SOURCE_SHA256 = '6866c877d39cba9c357620878839b336d569f8c662d3cfab4cb1dbe2d39c977f';
const [input] = process.argv.slice(2);
if (!input) throw new Error('Usage: node scripts/prepare-countries.mjs ne_110m_admin_0_countries.geojson');
const bytes = await readFile(input);
const sha = createHash('sha256').update(bytes).digest('hex');
if (sha !== SOURCE_SHA256) throw new Error(`unexpected Natural Earth source ${sha}`);
const source = JSON.parse(bytes);
// Three decimals (about 100 m) is far finer than the 1:110m linework.
const round = (value) => Array.isArray(value) ? value.map(round) : Math.round(value * 1000) / 1000;
// ISO_A2_EH fills the ISO_A2 gaps Natural Earth leaves for France and Norway.
// Kosovo's XK is not an ISO 3166-1 assignment, and N. Cyprus and Somaliland
// have none, so those outlines stay unmatched (iso null).
// The label point (zega#74): where an arc to or from the country lands. The
// pole of inaccessibility of the largest polygon (Mapbox's polylabel
// algorithm) sits inside the main landmass, where a centroid would fall in
// the sea for Norway, Chile or the United States with Alaska.
function ringArea(ring) {
  let area = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) area += (ring[j][0] + ring[i][0]) * (ring[j][1] - ring[i][1]);
  return Math.abs(area / 2);
}
function segmentDistanceSq(x, y, [ax, ay], [bx, by]) {
  let dx = bx - ax, dy = by - ay;
  if (dx || dy) {
    const t = ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy);
    if (t > 1) { ax = bx; ay = by; } else if (t > 0) { ax += dx * t; ay += dy * t; }
  }
  dx = x - ax; dy = y - ay;
  return dx * dx + dy * dy;
}
// Signed distance from a point to the polygon: positive inside.
function polygonDistance(x, y, polygon) {
  let inside = false, min = Infinity;
  for (const ring of polygon) {
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const a = ring[i], b = ring[j];
      if ((a[1] > y) !== (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]) inside = !inside;
      min = Math.min(min, segmentDistanceSq(x, y, a, b));
    }
  }
  return (inside ? 1 : -1) * Math.sqrt(min);
}
function polylabel(polygon, precision) {
  const xs = polygon[0].map((p) => p[0]), ys = polygon[0].map((p) => p[1]);
  const minX = Math.min(...xs), minY = Math.min(...ys), width = Math.max(...xs) - minX, height = Math.max(...ys) - minY;
  const cell = (x, y, h) => ({ x, y, h, d: polygonDistance(x, y, polygon) });
  const size = Math.min(width, height);
  if (!size) return [minX, minY];
  let h = size / 2;
  const cells = [];
  for (let x = minX; x < minX + width; x += size) for (let y = minY; y < minY + height; y += size) cells.push(cell(x + h, y + h, h));
  let best = cell(minX + width / 2, minY + height / 2, 0);
  while (cells.length) {
    let top = 0;
    for (let i = 1; i < cells.length; i++) if (cells[i].d + cells[i].h * Math.SQRT2 > cells[top].d + cells[top].h * Math.SQRT2) top = i;
    const c = cells.splice(top, 1)[0];
    if (c.d > best.d) best = c;
    if (c.d + c.h * Math.SQRT2 - best.d <= precision) continue;
    h = c.h / 2;
    cells.push(cell(c.x - h, c.y - h, h), cell(c.x + h, c.y - h, h), cell(c.x - h, c.y + h, h), cell(c.x + h, c.y + h, h));
  }
  return [best.x, best.y];
}
function labelPoint(geometry) {
  const polygons = geometry.type === 'Polygon' ? [geometry.coordinates] : geometry.coordinates;
  const main = polygons.reduce((a, b) => (ringArea(b[0]) > ringArea(a[0]) ? b : a));
  // Longitudes shrink by cos(latitude) so distances are even on the ground.
  const lat0 = main[0].reduce((sum, p) => sum + p[1], 0) / main[0].length;
  const k = Math.cos(lat0 * Math.PI / 180);
  const [x, y] = polylabel(main.map((ring) => ring.map(([lon, lat]) => [lon * k, lat])), 0.02);
  return [Math.round(x / k * 100) / 100, Math.round(y * 100) / 100];
}

const features = source.features.map(({ properties: p, geometry }) => ({
  type: 'Feature',
  properties: { iso: /^[A-Z]{2}$/.test(p.ISO_A2_EH) && p.ISO_A2_EH !== 'XK' ? p.ISO_A2_EH : null, name: p.NAME, label: labelPoint(geometry) },
  geometry: { type: geometry.type, coordinates: round(geometry.coordinates) },
}));
const out = fileURLToPath(new URL('../data/countries-110m.geojson', import.meta.url));
await writeFile(out, JSON.stringify({ type: 'FeatureCollection', features }) + '\n');
console.log(`Wrote ${features.length} countries to ${out}`);
