// Build the Flights sample from OpenFlights (https://openflights.org/data.php):
// airports, routes and countries under the ODbL 1.0, with their contents under
// the DbCL 1.0, pinned to github.com/jpatokal/openflights commit
// 7d1a611e070295dba776d6afb86e57d0d1aa1cef:
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/airports.dat
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/routes.dat
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/countries.dat
// Usage: node scripts/prepare-flights.mjs /path/to/openflights/data
import { format } from './format.mjs';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const SOURCES = {
  'airports.dat': '9387cdb38df5bd664da823f8ccb69fdd9b33a1888f5b7cca09c34a3cd9ff59f9',
  'routes.dat': 'bd373706238134f619c624c606dccc74c05c2582a977c489c81de501735f2390',
  'countries.dat': '5cbd1a7da0f4f8003f595d22d80f025f483caad7a0672354ef9bc70221d348ed',
};
// The busiest airport of each of the COUNTRIES busiest countries, the HUBS
// busiest airports overall, and HOME: a map of the world's hubs, not a web
// over Europe and the United States. "Busiest" is the number of distinct
// non-stop routes in the whole dataset.
const COUNTRIES = 45, HUBS = 15, HOME = 'YYC';

const [directory] = process.argv.slice(2);
if (!directory) throw new Error('Usage: node scripts/prepare-flights.mjs /path/to/openflights/data');
const hashes = {};
const files = {};
for (const [name, expected] of Object.entries(SOURCES)) {
  const bytes = await readFile(`${directory}/${name}`);
  hashes[name] = createHash('sha256').update(bytes).digest('hex');
  if (hashes[name] !== expected) throw new Error(`unexpected OpenFlights ${name}: ${hashes[name]}`);
  files[name] = bytes.toString('utf8');
}
// OpenFlights' .dat files are CSV with quoted text and \N for null.
function rows(text) {
  return text.split('\n').filter(Boolean).map((line) => {
    const out = [];
    let cell = '', quoted = false;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      if (ch === '"') {
        if (quoted && line[i + 1] === '"') { cell += '"'; i++; } else quoted = !quoted;
      } else if (ch === ',' && !quoted) { out.push(cell); cell = ''; } else cell += ch;
    }
    out.push(cell);
    return out.map((value) => (value === '\\N' ? null : value));
  });
}

const iso = new Map(rows(files['countries.dat']).map(([name, code]) => [name, code]));
const airports = new Map();
for (const [, name, city, country, code, , lat, lon, , , , , kind] of rows(files['airports.dat'])) {
  if (kind === 'airport' && /^[A-Z]{3}$/.test(code || '')) airports.set(code, { code, name, city, country, lat: Number(lat), lon: Number(lon) });
}
const pairs = new Set();
for (const [, , origin, , destination, , , stops] of rows(files['routes.dat'])) {
  if (stops === '0' && origin !== destination && airports.has(origin) && airports.has(destination)) pairs.add([origin, destination].sort().join('-'));
}
const degree = new Map();
for (const pair of pairs) for (const code of pair.split('-')) degree.set(code, (degree.get(code) || 0) + 1);
const ranked = [...degree.keys()].sort((a, b) => degree.get(b) - degree.get(a) || a.localeCompare(b));
const busiestOfCountry = new Map();
for (const code of ranked) {
  const { country } = airports.get(code);
  if (!busiestOfCountry.has(country)) busiestOfCountry.set(country, code);
}
const selected = new Set([...[...busiestOfCountry.values()].slice(0, COUNTRIES), ...ranked.slice(0, HUBS), HOME]);
for (const code of selected) {
  const { country } = airports.get(code);
  if (!iso.has(country)) throw new Error(`no ISO code for ${country} (${code})`);
}
// Each route once, from the smaller airport to the larger hub, so a hub's
// routes are its `inbound` ones and a spoke's are its `route`s.
const routes = [...pairs].filter((pair) => pair.split('-').every((code) => selected.has(code)))
  .map((pair) => pair.split('-').sort((a, b) => degree.get(a) - degree.get(b) || a.localeCompare(b)))
  .sort((a, b) => a[0].localeCompare(b[0]) || a[1].localeCompare(b[1]));
const chosen = [...selected].map((code) => airports.get(code)).sort((a, b) => a.code.localeCompare(b.code));
const countries = [...new Set(chosen.map((airport) => airport.country))].sort((a, b) => a.localeCompare(b, 'en'))
  .map((name) => ({ name, iso: iso.get(name) }));

const cell = (value) => `"${String(value).replaceAll('"', '""')}"`;
const csv = (header, records) => [header, ...records.map((record) => record.map(cell).join(','))].join('\n') + '\n';
// The provenance (source commit, SHA-256s, selection) is documented in
// browser/docs/display.md; the sample carries one line.
const source = `// Airports and routes: OpenFlights (ODbL 1.0), see browser/docs/display.md
schema {
  type Country {
    name: String
    iso: String<iso2>
    airports: BASE <- Airport[]
  }
  type Airport {
    code: String
    name: String
    city: String
    at: Point from (lat, lon)
    country: BASE -> Country
    route: ROUTE -> Airport[]
    inbound: ROUTE <- Airport[]
  }
  display {
    globe(@zoom: 1.0, @tilt: 40, @center: @point(55, -45)) { Airport }: Default
    map { Airport }
    table
    graph
  }
}
unique { Airport { code } Country { iso } }

mutation csv ["./samples/flights-countries.csv"] { Country(name: $name && iso: $iso) { name } }
mutation csv ["./samples/flights-airports.csv"] { Airport(code: $code && name: $name && city: $city) { code at } }
mutation csv ["./samples/flights-airports.csv"] { Airport(code: $code) { country -> link Country(iso: $country) } }
mutation csv ["./samples/flights-routes.csv"] { Airport(code: $origin) { route -> link Airport(code: $destination) } }
`;
const out = (name) => fileURLToPath(new URL(`../samples/${name}`, import.meta.url));
await writeFile(out('flights.zql'), format(source));
await writeFile(out('flights-countries.csv'), csv('name,iso', countries.map(({ name, iso }) => [name, iso])));
await writeFile(out('flights-airports.csv'), csv('code,name,city,country,lat,lon', chosen.map((a) => [a.code, a.name, a.city, iso.get(a.country), a.lat.toFixed(4), a.lon.toFixed(4)])));
await writeFile(out('flights-routes.csv'), csv('origin,destination', routes));
console.log(`Wrote ${chosen.length} airports in ${countries.length} countries and ${routes.length} routes; ${HOME} flies to ${routes.filter(([origin]) => origin === HOME).map(([, to]) => to).join(' ')}`);
