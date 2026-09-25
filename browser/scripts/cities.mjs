// The cities in the explorer's basemap archive and its multi-city sample.
// One list, shared by the tile extraction (prepare-tiles.mjs) and the sample
// generator (cities-sample.mjs), so the sample can only draw places from
// areas the archive actually covers.
//
// `bbox` is [min_lon, min_lat, max_lon, max_lat], a city-sized box around the
// centre, comparable to Calgary's. `anchor` names an OSM POI in the tiles (by its English name where OSM's is in a non-Latin script) that
// the sample's example queries measure from; `country` is the ISO 3166-1
// alpha-2 code the globe highlights.
export const CITIES = [
  { name: 'Calgary', country: 'CA', bbox: [-114.32, 50.84, -113.86, 51.21], anchor: 'Calgary Tower' },
  { name: 'New York', country: 'US', bbox: [-74.15, 40.62, -73.79, 40.89], anchor: 'Empire State Building' },
  { name: 'San Francisco', country: 'US', bbox: [-122.53, 37.64, -122.19, 37.91], anchor: 'San Francisco Ferry Building' },
  { name: 'London', country: 'GB', bbox: [-0.36, 51.37, 0.12, 51.64], anchor: 'British Museum' },
  { name: 'Rome', country: 'IT', bbox: [12.32, 41.76, 12.68, 42.03], anchor: 'Fontana di Trevi' },
  { name: 'Addis Ababa', country: 'ET', bbox: [38.63, 8.87, 38.90, 9.14], anchor: 'Adwa Victory Memorial Museum' },
  { name: 'Tokyo', country: 'JP', bbox: [139.58, 35.55, 139.91, 35.82], anchor: 'Tokyo Tower' },
  { name: 'Sydney', country: 'AU', bbox: [151.05, -34.00, 151.37, -33.73], anchor: 'Sydney Opera House' },
];

/** The extraction region: one rectangle per city, as a GeoJSON MultiPolygon. */
export function region() {
  return {
    type: 'MultiPolygon',
    coordinates: CITIES.map(({ bbox: [w, s, e, n] }) => [[[w, s], [e, s], [e, n], [w, n], [w, s]]]),
  };
}
