import { layers, namedFlavor } from './vendor/basemaps/index.js';
import { palettes } from './theme.js';
export const TILE_ORIGIN = 'https://tiles.zega.dev';
export const ATTRIBUTION = '© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">OpenStreetMap contributors</a>';
/** The Protomaps flavor recoloured with the explorer's theme tokens. */
export function basemapFlavor(theme) {
  const c = palettes[theme];
  const base = namedFlavor(theme);
  const flavor = { ...base };
  for (const key of Object.keys(base)) {
    if (typeof base[key] !== 'string') continue;
    flavor[key] = key.includes('halo') ? c.ground : key.includes('label') ? (key === 'city_label' || key === 'country_label' ? c.ink : c.soft)
      : /^(tunnel|bridges|minor|major|highway|link|other|railway|runway|boundaries|pier)/.test(key) ? (key.includes('casing') ? c.strongRule : c.rule)
      : /park|wood|scrub|zoo/.test(key) ? c.park : key === 'water' ? c.water : key === 'buildings' ? c.rule : c.ground;
  }
  flavor.landcover = Object.fromEntries(Object.keys(base.landcover).map((key) => [key, /forest|grass|scrub/.test(key) ? c.park : c.ground]));
  flavor.pois = Object.fromEntries(Object.keys(base.pois).map((key) => [key, c.soft]));
  return flavor;
}

export function mapStyle(theme, data, basemap = true, credit = '') {
  const c = palettes[theme];
  const flavor = basemapFlavor(theme);
  return {
    version: 8,
    ...(basemap ? { glyphs: `${TILE_ORIGIN}/fonts/{fontstack}/{range}.pbf`, sprite: `${TILE_ORIGIN}/sprites/v4/${theme}` } : {}),
    sources: {
      ...(basemap ? { basemap: { type: 'vector', url: `pmtiles://${TILE_ORIGIN}/calgary.pmtiles`, attribution: ATTRIBUTION } } : {}),
      'zega-nodes': { type: 'geojson', ...(credit ? { attribution: credit } : {}), data },
    },
    layers: [
      ...(basemap ? layers('basemap', flavor, { lang: 'en' }) : [{ id: 'ground', type: 'background', paint: { 'background-color': c.ground } }]),
      { id: 'zega-nodes', type: 'circle', source: 'zega-nodes', paint: { 'circle-radius': 7, 'circle-color': c.accent, 'circle-stroke-color': c.panel, 'circle-stroke-width': 2 } },
    ],
  };
}
