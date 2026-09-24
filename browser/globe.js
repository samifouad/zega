import * as maplibre from './vendor/maplibre-gl/maplibre-gl.mjs';
import { Protocol } from './vendor/pmtiles/index.js';
import { layers } from './vendor/basemaps/index.js';
import { ATTRIBUTION, TILE_ORIGIN, basemapFlavor } from './map-style.js';
import { palettes } from './theme.js';
import { ArcLayer } from './arcs.js';

// APS 9: a globe that becomes the flat map. MapLibre's `globe` projection
// renders a sphere at low zoom and blends into Mercator between zoom 10 and 12.
const protocol = new Protocol();
maplibre.addProtocol('pmtiles', protocol.tile);
export const COUNTRIES_URL = new URL('./data/countries-110m.geojson', import.meta.url).href;
const BASEMAP_MINZOOM = 5;
const NATURAL_EARTH = '<a href="https://www.naturalearthdata.com/" target="_blank" rel="noopener">Natural Earth</a>';

/** Globe inputs from the stored nodes of the view's types, and the relationships among them. */
export function globeData(nodes, types, rels = []) {
  const countries = new Map();
  const codes = new Map();
  const places = [];
  for (const node of nodes) {
    const type = types.find((type) => node.labels.includes(type.name));
    const props = type?.fields.filter((field) => field.kind === 'prop') || [];
    const iso = props.find((field) => field.ty === 'String<iso2>');
    const code = iso && node[iso.name];
    if (typeof code === 'string') {
      codes.set(node.id, code);
      if (!countries.has(code)) countries.set(code, node);
    }
    const point = props.find((field) => field.ty === 'Point' && node[field.name] != null);
    const at = point ? node[point.name] : node;
    if (Number.isFinite(at?.lat) && Number.isFinite(at?.lon)) places.push({ node, lat: at.lat, lon: at.lon });
  }
  return { countries, codes, places, rels };
}

// zega#74: the globe's own settings, kept like the graph's layout settings.
const GLOBE_KEY = 'zega.browser.globe';
export function globeDefaults() {
  return { lift: 0.18, spin: false, edges: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'static' : 'animated' };
}
function loadGlobeSettings() {
  try { return { ...globeDefaults(), ...JSON.parse(localStorage.getItem(GLOBE_KEY) || '{}') }; }
  catch { return globeDefaults(); }
}
function saveGlobeSettings(settings) {
  try { localStorage.setItem(GLOBE_KEY, JSON.stringify(settings)); } catch { /* private mode */ }
}

// A gear in the map's control stack that opens the settings panel.
class SettingsControl {
  constructor(settings, apply) {
    this.settings = settings;
    this.apply = apply;
  }
  onAdd() {
    const container = document.createElement('div');
    container.className = 'maplibregl-ctrl maplibregl-ctrl-group globe-settings-ctrl';
    const gear = document.createElement('button');
    gear.type = 'button';
    gear.className = 'globe-gear';
    gear.title = 'globe settings';
    gear.setAttribute('aria-label', 'globe settings');
    gear.setAttribute('aria-expanded', 'false');
    gear.textContent = '⚙';
    const panel = document.createElement('div');
    panel.className = 'graph-physics globe-settings';
    panel.innerHTML = `
      <div class="ph-row"><span>Lift</span><input type="range" data-p="lift" min="0" max="0.5" step="0.01" aria-label="lift"><b></b></div>
      <div class="ph-row"><span>Edges</span><select data-p="edges" aria-label="edges"><option value="animated">animated</option><option value="static">static</option></select><b></b></div>
      <div class="ph-row"><span>Auto-spin</span><input type="checkbox" data-p="spin" aria-label="auto-spin"><b></b></div>
      <div class="ph-foot"><button type="button" class="mini">reset</button></div>`;
    gear.onclick = () => {
      const open = panel.classList.toggle('open');
      gear.setAttribute('aria-expanded', String(open));
    };
    const inputs = [...panel.querySelectorAll('[data-p]')];
    const show = () => {
      for (const input of inputs) {
        const key = input.dataset.p, value = this.settings[key];
        if (input.type === 'checkbox') input.checked = value;
        else input.value = String(value);
        input.parentElement.querySelector('b').textContent = key === 'lift' ? value.toFixed(2) : '';
      }
    };
    show();
    for (const input of inputs) {
      input.addEventListener(input.tagName === 'SELECT' ? 'change' : 'input', () => {
        const key = input.dataset.p;
        this.settings[key] = input.type === 'checkbox' ? input.checked : input.type === 'range' ? Number(input.value) : input.value;
        show();
        this.apply(key);
      });
    }
    panel.querySelector('.ph-foot button').onclick = () => {
      Object.assign(this.settings, globeDefaults());
      show();
      for (const key of Object.keys(this.settings)) this.apply(key);
    };
    container.append(gear, panel);
    this.container = container;
    return container;
  }
  onRemove() { this.container.remove(); }
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
      // The arcs layer (zega#74) is added here, under the markers, once the style loads.
      { id: 'zega-nodes', type: 'circle', source: 'zega-nodes', paint: { 'circle-radius': 6, 'circle-color': c.accent, 'circle-stroke-color': c.panel, 'circle-stroke-width': 2 } },
    ],
  };
}

export function renderGlobe(container, { countries, codes, places, rels = [] }, camera, theme, onNode, onEdge) {
  const root = document.createElement('div');
  root.className = 'map-view globe-view';
  const canvas = document.createElement('div');
  canvas.className = 'map-canvas';
  canvas.setAttribute('aria-label', 'Globe of countries, places and relationships');
  const notice = document.createElement('div');
  notice.className = 'map-notice';
  notice.setAttribute('role', 'status');
  notice.hidden = true;
  const count = document.createElement('div');
  count.className = 'map-count';
  root.append(canvas, notice, count);
  container.replaceChildren(root);
  const codeList = [...countries.keys()];
  let plain = false;
  let outlines = { type: 'FeatureCollection', features: [] };
  const show = (text) => { notice.textContent = text; notice.hidden = false; };
  const map = new maplibre.Map({
    container: canvas,
    style: globeStyle(theme, codeList, places, outlines, true),
    center: [camera.center.lon, camera.center.lat],
    zoom: camera.zoom,
    pitch: camera.tilt,
    maxPitch: 85,
    attributionControl: false,
  });
  map.addControl(new maplibre.AttributionControl({ compact: false, customAttribution: ATTRIBUTION }), 'bottom-right');
  map.addControl(new maplibre.NavigationControl({ showCompass: false }), 'top-right');

  // Relationships as arcs (zega#74): every relationship among the view's
  // nodes whose ends both have a location. Places use their point; countries
  // use the label point that ships with the outlines.
  const settings = loadGlobeSettings();
  const arcs = new ArcLayer({ color: palettes[theme].accent, lift: settings.lift, animate: settings.edges === 'animated' });
  const placeAt = new Map(places.map(({ node, lat, lon }) => [node.id, [lon, lat]]));
  let labels = new Map();
  const locate = (id) => placeAt.get(id) || labels.get(codes.get(id)) || null;
  function updateArcs() {
    const records = rels.flatMap((rel) => {
      const from = locate(rel.from), to = locate(rel.to);
      return from && to ? [{ from, to, rel }] : [];
    });
    arcs.setArcs(records);
    const n = countries.size, m = places.length, k = records.length;
    count.textContent = `${n} ${n === 1 ? 'country' : 'countries'} · ${m} ${m === 1 ? 'place' : 'places'}`
      + (k ? ` · ${k} ${k === 1 ? 'relationship' : 'relationships'}` : '');
  }
  updateArcs();
  map.on('style.load', () => { if (!map.getLayer(arcs.id)) map.addLayer(arcs, 'zega-nodes'); });

  // Auto-spin: the preview's slow eastward turn, slowing as the map zooms in
  // and stopping as it flattens; never while the reader is dragging.
  let interacting = false, spinFrame = 0, spinLast = 0;
  const spin = (now) => {
    spinFrame = 0;
    if (!settings.spin || !container._map) return;
    const dt = spinLast ? Math.min(now - spinLast, 50) : 16.7;
    spinLast = now;
    if (!interacting) {
      const globeness = map.style?.projection?.transitionState ?? 1;
      const rate = 0.06 * Math.min(1, 2 ** (1.5 - map.getZoom())) * globeness * dt / 16.7;
      const c = map.getCenter();
      if (rate) map.jumpTo({ center: [c.lng - rate, c.lat] });
    }
    spinFrame = requestAnimationFrame(spin);
  };
  const startSpin = () => { spinLast = 0; if (!spinFrame) spinFrame = requestAnimationFrame(spin); };
  for (const event of ['mousedown', 'touchstart', 'wheel']) map.on(event, () => { interacting = true; });
  for (const event of ['mouseup', 'touchend', 'dragend', 'zoomend']) map.on(event, () => { interacting = false; });
  const apply = (key) => {
    saveGlobeSettings(settings);
    if (key === 'lift') arcs.setSettings({ lift: settings.lift });
    if (key === 'edges') arcs.setSettings({ animate: settings.edges === 'animated' });
    if (key === 'spin') startSpin();
  };
  map.addControl(new SettingsControl(settings, apply), 'top-right');
  if (settings.spin) startSpin();

  // The outlines are a static explorer asset, fetched once per view.
  const loaded = fetch(COUNTRIES_URL).then((response) => {
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  }).then((data) => {
    outlines = data;
    labels = new Map(data.features.filter((feature) => feature.properties.iso && feature.properties.label).map((feature) => [feature.properties.iso, feature.properties.label]));
    updateArcs();
    const apply = () => map.getSource('countries').setData(data);
    if (map.getSource('countries')) apply(); else map.once('load', apply);
  }).catch(() => show('Country outlines unavailable. Your places are still shown.'));
  map.on('error', () => {
    if (plain) return;
    plain = true;
    show('Base map unavailable. Countries and places are still shown.');
    map.setStyle(globeStyle(theme, codeList, places, outlines, false));
  });
  const nearest = (event) => [...event.features].sort((a, b) => {
    const distance = (feature) => { const p = map.project(feature.geometry.coordinates); return Math.hypot(p.x - event.point.x, p.y - event.point.y); };
    return distance(a) - distance(b);
  })[0];
  // Places sit above arcs, which sit above countries: a click on a marker
  // opens the place, on an arc the relationship, on a highlight the country.
  map.on('click', async (event) => {
    const hits = map.queryRenderedFeatures(event.point, { layers: ['zega-nodes', 'globe-countries'].filter((id) => map.getLayer(id)) });
    const place = hits.filter((feature) => feature.layer.id === 'zega-nodes');
    if (place.length) {
      const id = nearest({ ...event, features: place })?.properties.id;
      const found = places.find(({ node }) => node.id === id);
      if (found) return onNode(found.node);
    }
    const rel = await arcs.pick(event.point.x, event.point.y);
    if (rel) return onEdge?.(rel);
    const country = hits.find((feature) => feature.layer.id === 'globe-countries');
    const node = country && countries.get(country.properties.iso);
    if (node) onNode(node);
  });
  let overLayer = 0, overArc = false, hover = 0;
  const cursor = () => { map.getCanvas().style.cursor = overLayer || overArc ? 'pointer' : ''; };
  for (const layer of ['zega-nodes', 'globe-countries']) {
    map.on('mouseenter', layer, () => { overLayer++; cursor(); });
    map.on('mouseleave', layer, () => { overLayer = Math.max(0, overLayer - 1); cursor(); });
  }
  map.on('mousemove', (event) => {
    if (hover || map.isMoving() || !arcs.count) return;
    hover = setTimeout(async () => {
      hover = 0;
      overArc = !!(await arcs.pick(event.point.x, event.point.y));
      cursor();
    }, 80);
  });
  const resize = new ResizeObserver(() => map.resize());
  resize.observe(canvas);
  container._map = map;
  container._arcs = arcs;
  container._outlines = loaded;
  return () => {
    resize.disconnect();
    clearTimeout(hover);
    cancelAnimationFrame(spinFrame);
    map.remove();
    container._map = null;
    container._arcs = null;
  };
}
