// OpenFlights (https://openflights.org/data.php), read and verified once for
// every sample built from it (prepare-flights.mjs, cities-sample.mjs):
// airports, routes and countries under the ODbL 1.0, with their contents under
// the DbCL 1.0, pinned to github.com/jpatokal/openflights commit
// 7d1a611e070295dba776d6afb86e57d0d1aa1cef:
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/airports.dat
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/routes.dat
//   https://raw.githubusercontent.com/jpatokal/openflights/7d1a611e070295dba776d6afb86e57d0d1aa1cef/data/countries.dat
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';

export const SOURCES = {
  'airports.dat': '9387cdb38df5bd664da823f8ccb69fdd9b33a1888f5b7cca09c34a3cd9ff59f9',
  'routes.dat': 'bd373706238134f619c624c606dccc74c05c2582a977c489c81de501735f2390',
  'countries.dat': '5cbd1a7da0f4f8003f595d22d80f025f483caad7a0672354ef9bc70221d348ed',
};

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

/**
 * The pinned OpenFlights files from `directory`, refusing any other bytes:
 * `iso` maps a country name to its ISO 3166-1 alpha-2 code, `airports` maps an
 * IATA code to its airport, and `pairs` holds each non-stop route once, as the
 * two codes sorted and joined by `-`.
 */
export async function readOpenFlights(directory) {
  const files = {};
  for (const [name, expected] of Object.entries(SOURCES)) {
    const bytes = await readFile(`${directory}/${name}`);
    const hash = createHash('sha256').update(bytes).digest('hex');
    if (hash !== expected) throw new Error(`unexpected OpenFlights ${name}: ${hash}`);
    files[name] = bytes.toString('utf8');
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
  return { iso, airports, pairs };
}
