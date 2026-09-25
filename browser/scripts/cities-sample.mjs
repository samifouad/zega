// Build the Calgary and Cities samples from the explorer's own basemap
// archive (prepare-tiles.mjs) and OpenFlights (openflights.mjs).
// No coordinates are authored here: a place is an OSM POI label point from
// the archive's z15 `pois` layer (polygon POIs use Protomaps' label point),
// and a city is its OSM label point from the archive's `places` layer.
// Usage: node scripts/cities-sample.mjs ../.tmp/tiles/cities.pmtiles ../.tmp/openflights
import { format } from './format.mjs';
import { CITIES } from './cities.mjs';
import { readOpenFlights } from './openflights.mjs';
import { open, writeFile, mkdir } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { PMTiles } from 'pmtiles';
import { VectorTile } from '@mapbox/vector-tile';
import { PbfReader } from 'pbf';

const [input, flights] = process.argv.slice(2);
if (!input || !flights) throw new Error('Usage: node scripts/cities-sample.mjs ../.tmp/tiles/cities.pmtiles /path/to/openflights/data');
const file = await open(input);
const archive = new PMTiles({
  getKey: () => input,
  async getBytes(offset, length) {
    const buffer = Buffer.alloc(length);
    const { bytesRead } = await file.read(buffer, 0, length, offset);
    if (bytesRead !== length) throw new Error('Truncated PMTiles');
    return { data: buffer.buffer };
  },
});
const header = await archive.getHeader();
if (header.maxZoom !== 15) throw new Error('Expected a maxzoom 15 extract');
const inside = ([w, s, e, n], lon, lat) => lon >= w && lon <= e && lat >= s && lat <= n;

// Every feature of `layer` at zoom z over the box, as GeoJSON.
async function features(bbox, z, layer) {
  const tileX = (lon) => Math.floor((lon + 180) / 360 * 2 ** z);
  const tileY = (lat) => Math.floor((1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * 2 ** z);
  const out = [];
  for (let x = tileX(bbox[0]); x <= tileX(bbox[2]); x++) {
    for (let y = tileY(bbox[3]); y <= tileY(bbox[1]); y++) {
      const tile = await archive.getZxy(z, x, y);
      if (!tile) continue;
      const found = new VectorTile(new PbfReader(new Uint8Array(tile.data))).layers[layer];
      for (let i = 0; i < (found?.length || 0); i++) out.push(found.feature(i).toGeoJSON(x, y, z));
    }
  }
  return out;
}

// A city's label point: the OSM locality named for it (in English where the
// local name differs), inside its box. Zoom 9 is the archive's first zoom
// that carries every one of these cities' labels.
async function cityPoint(city) {
  const labels = (await features(city.bbox, 9, 'places')).filter((f) => f.properties.kind === 'locality'
    && (f.properties['name:en'] ?? f.properties.name) === city.name && inside(city.bbox, ...f.geometry.coordinates));
  if (!labels.length) throw new Error(`No OSM label for ${city.name}`);
  const [lon, lat] = labels[0].geometry.coordinates;
  return { name: city.name, country: city.country, lat, lon };
}

// A city's named POI label points, keyed by feature id.
async function cityPois(city) {
  const points = new Map();
  for (const f of await features(city.bbox, 15, 'pois')) {
    // The OSM name, or its English one where the name is in a non-Latin
    // script, so a reader can tell Tokyo's places apart.
    const local = f.properties.name;
    const name = local && /[^\p{Script=Latin}\p{Script=Common}\p{Script=Inherited}]/u.test(local) ? f.properties['name:en'] ?? local : local;
    if (f.geometry.type !== 'Point' || !name) continue;
    const [lon, lat] = f.geometry.coordinates;
    if (!inside(city.bbox, lon, lat)) continue;
    points.set(f.id, { name, kind: f.properties.kind, lon, lat, tile_id: f.id, city: city.name });
  }
  return [...points.values()];
}

// The places of each kind nearest the city's anchor POI, `count` of each,
// skipping names already `used` (a name identifies a place in a sample).
function select(city, all, groups, used) {
  const anchor = all.find((p) => p.name === city.anchor);
  if (!anchor) throw new Error(`${city.anchor} missing from source`);
  const distance = (p) => (p.lat - anchor.lat) ** 2 + ((p.lon - anchor.lon) * Math.cos(anchor.lat * Math.PI / 180)) ** 2;
  const selected = [];
  for (const [kinds, count] of groups) {
    const seen = new Set();
    const candidates = all.filter((p) => kinds.includes(p.kind)).sort((a, b) => distance(a) - distance(b) || a.tile_id - b.tile_id)
      .filter((p) => { if (seen.has(p.name) || used.has(p.name)) return false; seen.add(p.name); return true; }).slice(0, count);
    if (candidates.length !== count) throw new Error(`${city.name}: need ${count} ${kinds}; found ${candidates.length}`);
    for (const p of candidates) used.add(p.name);
    selected.push(...candidates);
  }
  return { anchor, selected };
}
const SIGHTS = ['attraction', 'artwork', 'monument', 'memorial', 'viewpoint'];

const hash = createHash('sha256');
for await (const bytes of createReadStream(input)) hash.update(bytes);
const sourceHash = hash.digest('hex');
// Quoted only where CSV needs it.
const cell = (value) => (/[",\n]/.test(String(value)) ? `"${String(value).replaceAll('"', '""')}"` : String(value));
const csv = (header, records) => [header, ...records.map((record) => record.map(cell).join(','))].join('\n') + '\n';
const byName = (a, b) => a.name.localeCompare(b.name, 'en');
await mkdir('samples', { recursive: true });

const cities = [], places = [];
const used = new Set();
let calgary;
try {
  for (const city of CITIES) {
    const pois = await cityPois(city);
    cities.push(await cityPoint(city));
    places.push(...select(city, pois, [[['park'], 3], [['museum'], 3], [SIGHTS, 4], [['cafe'], 2]], used).selected);
    // The Calgary sample: the same selection it has always had, 30 places.
    if (city.name === 'Calgary') calgary = select(city, pois, [[['park'], 10], [['museum'], 6], [SIGHTS, 10], [['cafe'], 4]], new Set());
  }
} finally { await file.close(); }

// Calgary: one city, its places, and a radius query around the Calgary Tower.
const origin = `@point(${calgary.anchor.lat}, ${calgary.anchor.lon})`;
await writeFile('samples/calgary.zql', format(`// Calgary places — © OpenStreetMap contributors, ODbL 1.0.
// https://www.openstreetmap.org/copyright
// Generated by browser/scripts/cities-sample.mjs; coordinates are Protomaps POI label points.
// PMTiles SHA-256: ${sourceHash}
schema {
  type Place {
    name: String
    kind: String
    at: Point from (lat, lon)
  }
  display { map { Place }: Default table { Place } graph }
}
unique { Place { name } }

mutation csv ["./samples/calgary.csv"] { Place(name: $name && kind: $kind) { name kind at } }

// Places within 1.5 km of the Calgary Tower
query { Place(@distance(at, ${origin}) <= 1500) order by @distance(at, ${origin}) { @id name kind at @distance(at, ${origin}) } }
`));
await writeFile('samples/calgary.csv', csv('name,kind,lat,lon', calgary.selected.sort(byName).map((p) => [p.name, p.kind, p.lat, p.lon])));

// Cities: every city in cities.mjs with its places, and the non-stop flights
// between them. A city flies to another when OpenFlights has a non-stop route
// between an airport of each: one whose city is the city's name, in the
// city's country. Each route is stored once, from the city with fewer routes
// to the one with more (then by name), so a city's routes are its `route`s
// and `inbound` ones together.
const { iso, airports, pairs } = await readOpenFlights(flights);
const cityOf = new Map();
for (const airport of airports.values()) {
  const city = CITIES.find((city) => city.name === airport.city && city.country === iso.get(airport.country));
  if (city) cityOf.set(airport.code, city.name);
}
const flown = new Set();
for (const pair of pairs) {
  const [a, b] = pair.split('-').map((code) => cityOf.get(code));
  if (a && b && a !== b) flown.add([a, b].sort().join('\n'));
}
const degree = new Map();
for (const pair of flown) for (const name of pair.split('\n')) degree.set(name, (degree.get(name) || 0) + 1);
const routes = [...flown].map((pair) => pair.split('\n').sort((a, b) => degree.get(a) - degree.get(b) || a.localeCompare(b, 'en')))
  .sort(([a, b], [c, d]) => a.localeCompare(c, 'en') || b.localeCompare(d, 'en'));
for (const city of CITIES) if (!degree.has(city.name)) throw new Error(`${city.name} has no route to another city`);

await writeFile('samples/cities.zql', format(`// Cities: places © OpenStreetMap contributors (ODbL 1.0), https://www.openstreetmap.org/copyright;
// routes OpenFlights (ODbL 1.0). See browser/docs/tiles.md.
// Generated by browser/scripts/cities-sample.mjs; coordinates are Protomaps label points.
// PMTiles SHA-256: ${sourceHash}
schema {
  type City {
    name: String
    country: String<iso2>
    at: Point from (lat, lon)
    route: ROUTE -> City[]
    inbound: ROUTE <- City[]
    places: IN <- Place[]
  }
  type Place {
    name: String
    kind: String
    at: Point from (lat, lon)
    city: IN -> City
  }
  display {
    globe(@zoom: 0.85, @tilt: 10, @center: @point(55, -25)) { City Place }: Default
    map { City Place }
    table
    graph
  }
}
unique { City { name } Place { name } }

mutation csv ["./samples/cities-cities.csv"] { City(name: $name && country: $country) { name at } }
mutation csv ["./samples/cities-places.csv"] { Place(name: $name && kind: $kind) { name at } }
mutation csv ["./samples/cities-places.csv"] { Place(name: $name) { city -> link City(name: $city) } }
mutation csv ["./samples/cities-routes.csv"] { City(name: $from) { route -> link City(name: $to) } }
`));
await writeFile('samples/cities-cities.csv', csv('name,country,lat,lon', cities.map((c) => [c.name, c.country, c.lat, c.lon])));
await writeFile('samples/cities-places.csv', csv('name,kind,city,lat,lon', places.map((p) => [p.name, p.kind, p.city, p.lat, p.lon])));
await writeFile('samples/cities-routes.csv', csv('from,to', routes));
console.log(`PMTiles SHA-256 ${sourceHash}`);
console.log(`Calgary: ${calgary.selected.length} places. Cities: ${cities.length} cities, ${places.length} places, ${routes.length} routes`);
for (const city of cities) console.log(`${city.name} (${city.lat.toFixed(4)}, ${city.lon.toFixed(4)}): ${places.filter((p) => p.city === city.name).map((p) => p.name).join('; ')}`);
console.log(routes.map(([a, b]) => `${a} -> ${b}`).join('\n'));
