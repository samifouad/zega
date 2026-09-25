import * as maplibre from './vendor/maplibre-gl/maplibre-gl.mjs';
import { Protocol } from './vendor/pmtiles/index.js';
import { ATTRIBUTION, mapStyle } from './map-style.js';
const protocol = new Protocol();
maplibre.addProtocol('pmtiles', protocol.tile);

export function renderMap(container, nodes, theme, onNode, credit = '') {
  const root = document.createElement('div');
  root.className = 'map-view';
  const canvas = document.createElement('div');
  canvas.className = 'map-canvas';
  canvas.setAttribute('aria-label', 'Map of query results');
  const notice = document.createElement('div');
  notice.className = 'map-notice';
  notice.setAttribute('role', 'status');
  notice.hidden = true;
  const count = document.createElement('div');
  count.className = 'map-count';
  const plotted = nodes.filter((node) => Number.isFinite(node.lat) && Number.isFinite(node.lon));
  count.textContent = `${plotted.length} places`;
  root.append(canvas, notice, count);
  container.replaceChildren(root);
  const data = { type: 'FeatureCollection', features: plotted.map((node) => ({
    type: 'Feature', id: node.id, properties: { id: node.id, name: node.name || node.title || String(node.id) },
    geometry: { type: 'Point', coordinates: [node.lon, node.lat] },
  })) };
  let plain = false;
  let map;
  try {
    map = new maplibre.Map({ container: canvas, style: mapStyle(theme, data, true, credit), center: [-114.07, 51.05], zoom: 11, attributionControl: false });
  } catch (error) {
    // No WebGL2: an in-view notice, as for a failed basemap (zega#83).
    if (!/WebGL/.test(String(error?.message))) throw error;
    count.remove();
    notice.textContent = 'WebGL2 unavailable. This browser cannot draw the map.';
    notice.hidden = false;
    return () => {};
  }
  // A custom attribution remains visible even after a failed basemap is removed.
  map.addControl(new maplibre.AttributionControl({ compact: false, customAttribution: ATTRIBUTION }), 'bottom-right');
  map.addControl(new maplibre.NavigationControl({ showCompass: false }), 'top-right');
  map.on('error', () => {
    if (plain) return;
    plain = true;
    notice.textContent = 'Base map unavailable. Your places are still shown.';
    notice.hidden = false;
    map.setStyle(mapStyle(theme, data, false, credit));
  });
  if (plotted.length) {
    const bounds = new maplibre.LngLatBounds();
    plotted.forEach((node) => bounds.extend([node.lon, node.lat]));
    map.fitBounds(bounds, { padding: 55, maxZoom: 14, duration: 0 });
  }
  map.on('click', 'zega-nodes', (event) => {
    // Dense city markers overlap: choose the point nearest the click, rather
    // than whichever feature happens to be last in the source's draw order.
    const distance = (feature) => {
      const point = map.project(feature.geometry.coordinates);
      return Math.hypot(point.x - event.point.x, point.y - event.point.y);
    };
    const feature = [...event.features].sort((a, b) => distance(a) - distance(b))[0];
    const node = plotted.find((node) => node.id === feature?.properties.id);
    if (node) onNode(node);
  });
  map.on('mouseenter', 'zega-nodes', () => { map.getCanvas().style.cursor = 'pointer'; });
  map.on('mouseleave', 'zega-nodes', () => { map.getCanvas().style.cursor = ''; });
  const resize = new ResizeObserver(() => map.resize());
  resize.observe(canvas);
  // The native MapLibre instance is also useful from the browser console.
  container._map = map;
  return () => { resize.disconnect(); map.remove(); container._map = null; };
}
