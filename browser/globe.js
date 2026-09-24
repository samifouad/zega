import * as maplibre from './vendor/maplibre-gl/maplibre-gl.mjs';
import { Protocol } from './vendor/pmtiles/index.js';
import { layers } from './vendor/basemaps/index.js';
import { ATTRIBUTION, TILE_ORIGIN, basemapFlavor } from './map-style.js';
import { palettes } from './theme.js';

// APS 9: a globe that becomes the flat map. MapLibre's `globe` projection
// renders a sphere at low zoom and blends into Mercator between zoom 10 and 12.
const protocol = new Protocol();
maplibre.addProtocol('pmtiles', protocol.tile);
export const COUNTRIES_URL = new URL('./data/countries-110m.geojson', import.meta.url).href;
const BASEMAP_MINZOOM = 5;
const NATURAL_EARTH = '<a href="https://www.naturalearthdata.com/" target="_blank" rel="noopener">Natural Earth</a>';

/** Globe inputs from the stored nodes of the view's types. */
export function globeData(nodes, types) {
  const countries = new Map();
  const places = [];
  for (const node of nodes) {
    const type = types.find((type) => node.labels.includes(type.name));
    const props = type?.fields.filter((field) => field.kind === 'prop') || [];
    const iso = props.find((field) => field.ty === 'String<iso2>');
    const code = iso && node[iso.name];
    if (typeof code === 'string' && !countries.has(code)) countries.set(code, node);
    const point = props.find((field) => field.ty === 'Point' && node[field.name] != null);
    const at = point ? node[point.name] : node;
    if (Number.isFinite(at?.lat) && Number.isFinite(at?.lon)) places.push({ node, lat: at.lat, lon: at.lon });
  }
  return { countries, places };
}

function globeStyle(theme, codes, places, outlines, basemap) {
  const c = palettes[theme];
  const matched = ['in', ['get', 'iso'], ['literal', codes]];
  return {
    version: 8,
    projection: { type: 'globe' },
    sky: { 'atmosphere-blend': ['interpolate', ['linear'], ['zoom'], 0, 0.6, 5, 0.3, 8, 0] },
    ...(basemap ? { glyphs: `${TILE_ORIGIN}/fonts/{fontstack}/{range}.pbf`, sprite: `${TILE_ORIGIN}/sprites/v4/${theme}` } : {}),
    sources: {
      ...(basemap ? { basemap: { type: 'vector', url: `pmtiles://${TILE_ORIGIN}/calgary.pmtiles`, attribution: ATTRIBUTION } } : {}),
      countries: { type: 'geojson', data: outlines, attribution: NATURAL_EARTH },
      'zega-nodes': { type: 'geojson', data: {
        type: 'FeatureCollection',
        features: places.map(({ node, lat, lon }) => ({ type: 'Feature', properties: { id: node.id }, geometry: { type: 'Point', coordinates: [lon, lat] } })),
      } },
    },
    layers: [
      { id: 'globe-water', type: 'background', paint: { 'background-color': c.water } },
      // Natural Earth land under the basemap: the whole world at every zoom,
      // including when the basemap is unavailable.
      { id: 'globe-land', type: 'fill', source: 'countries', paint: { 'fill-color': c.ground } },
      // The OSM basemap joins in as the globe flattens; the sphere itself
      // stays the clean Natural Earth land and water at every theme.
      ...(basemap ? layers('basemap', basemapFlavor(theme), { lang: 'en' }).filter((layer) => layer.id !== 'background')
        .map((layer) => ({ ...layer, minzoom: Math.max(layer.minzoom ?? 0, BASEMAP_MINZOOM) })) : []),
      { id: 'globe-borders', type: 'line', source: 'countries', paint: { 'line-color': c.strongRule, 'line-width': 0.6 } },
      { id: 'globe-countries', type: 'fill', source: 'countries', filter: matched, paint: { 'fill-color': c.accent, 'fill-opacity': 0.55 } },
      { id: 'globe-countries-edge', type: 'line', source: 'countries', filter: matched, paint: { 'line-color': c.accent, 'line-width': 1.4 } },
      { id: 'zega-nodes', type: 'circle', source: 'zega-nodes', paint: { 'circle-radius': 6, 'circle-color': c.accent, 'circle-stroke-color': c.panel, 'circle-stroke-width': 2 } },
    ],
  };
}

export function renderGlobe(container, { countries, places }, camera, theme, onNode) {
  const root = document.createElement('div');
  root.className = 'map-view globe-view';
  const canvas = document.createElement('div');
  canvas.className = 'map-canvas';
  canvas.setAttribute('aria-label', 'Globe of countries and places');
  const notice = document.createElement('div');
  notice.className = 'map-notice';
  notice.setAttribute('role', 'status');
  notice.hidden = true;
  const count = document.createElement('div');
  count.className = 'map-count';
  const n = countries.size, m = places.length;
  count.textContent = `${n} ${n === 1 ? 'country' : 'countries'} · ${m} ${m === 1 ? 'place' : 'places'}`;
  root.append(canvas, notice, count);
  container.replaceChildren(root);
  const codes = [...countries.keys()];
  let plain = false;
  let outlines = { type: 'FeatureCollection', features: [] };
  const show = (text) => { notice.textContent = text; notice.hidden = false; };
  const map = new maplibre.Map({
    container: canvas,
    style: globeStyle(theme, codes, places, outlines, true),
    center: [camera.center.lon, camera.center.lat],
    zoom: camera.zoom,
    pitch: camera.tilt,
    maxPitch: 85,
    attributionControl: false,
  });
  map.addControl(new maplibre.AttributionControl({ compact: false, customAttribution: ATTRIBUTION }), 'bottom-right');
  map.addControl(new maplibre.NavigationControl({ showCompass: false }), 'top-right');
  // The outlines are a static explorer asset, fetched once per view.
  const loaded = fetch(COUNTRIES_URL).then((response) => {
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  }).then((data) => {
    outlines = data;
    const apply = () => map.getSource('countries').setData(data);
    if (map.getSource('countries')) apply(); else map.once('load', apply);
  }).catch(() => show('Country outlines unavailable. Your places are still shown.'));
  map.on('error', () => {
    if (plain) return;
    plain = true;
    show('Base map unavailable. Countries and places are still shown.');
    map.setStyle(globeStyle(theme, codes, places, outlines, false));
  });
  const nearest = (event) => [...event.features].sort((a, b) => {
    const distance = (feature) => { const p = map.project(feature.geometry.coordinates); return Math.hypot(p.x - event.point.x, p.y - event.point.y); };
    return distance(a) - distance(b);
  })[0];
  // Places sit above countries: a click on a marker opens the place.
  map.on('click', (event) => {
    const hits = map.queryRenderedFeatures(event.point, { layers: ['zega-nodes', 'globe-countries'].filter((id) => map.getLayer(id)) });
    const place = hits.filter((feature) => feature.layer.id === 'zega-nodes');
    if (place.length) {
      const id = nearest({ ...event, features: place })?.properties.id;
      const found = places.find(({ node }) => node.id === id);
      if (found) return onNode(found.node);
    }
    const country = hits.find((feature) => feature.layer.id === 'globe-countries');
    const node = country && countries.get(country.properties.iso);
    if (node) onNode(node);
  });
  for (const layer of ['zega-nodes', 'globe-countries']) {
    map.on('mouseenter', layer, () => { map.getCanvas().style.cursor = 'pointer'; });
    map.on('mouseleave', layer, () => { map.getCanvas().style.cursor = ''; });
  }
  const resize = new ResizeObserver(() => map.resize());
  resize.observe(canvas);
  container._map = map;
  container._outlines = loaded;
  return () => { resize.disconnect(); map.remove(); container._map = null; };
}
