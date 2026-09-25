// The map surface's street-map archives, apart from map.js so the registry
// and validator can check a contract's `basemap` values without loading
// MapLibre. `cities` is the explorer's shared archive on the tile host. The
// others are small same-origin extracts for places it does not cover, read
// whole (so any static server works: no HTTP range support needed).
import { BASEMAP } from '../../map-style.js';

export const ARCHIVES = Object.freeze({
  cities: Object.freeze({ url: BASEMAP }),
  // browser/scripts/circuit-sample.mjs: z13-15 over the Circuit de Monaco, 660 KB.
  monaco: Object.freeze({ url: new URL('../../data/monaco.pmtiles', import.meta.url).href, bounds: Object.freeze([7.405, 43.723, 7.440, 43.748]), local: true }),
});
/** What a contract's `basemap` setting may offer: an archive, or "auto". */
export const BASEMAP_VALUES = Object.freeze(['auto', ...Object.keys(ARCHIVES)]);

const contains = ([w, s, e, n], [bw, bs, be, bn]) => bw >= w && be <= e && bs >= s && bn <= n;
/** The archive for `basemap` ("auto" picks a local extract covering `bounds`, else cities). */
export function pickArchive(basemap, bounds) {
  if (basemap !== 'auto') return basemap;
  return Object.entries(ARCHIVES).find(([, a]) => a.bounds && bounds && contains(a.bounds, bounds))?.[0] || 'cities';
}
// Web Mercator's latitude limit: the map cannot show a point beyond it.
export const MERCATOR_LAT = 85.0511;
