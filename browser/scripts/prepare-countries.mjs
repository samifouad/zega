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
const features = source.features.map(({ properties: p, geometry }) => ({
  type: 'Feature',
  properties: { iso: /^[A-Z]{2}$/.test(p.ISO_A2_EH) && p.ISO_A2_EH !== 'XK' ? p.ISO_A2_EH : null, name: p.NAME },
  geometry: { type: geometry.type, coordinates: round(geometry.coordinates) },
}));
const out = fileURLToPath(new URL('../data/countries-110m.geojson', import.meta.url));
await writeFile(out, JSON.stringify({ type: 'FeatureCollection', features }) + '\n');
console.log(`Wrote ${features.length} countries to ${out}`);
