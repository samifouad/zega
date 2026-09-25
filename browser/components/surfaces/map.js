// The map surface: the explorer's street map (map-style.js, theme.js) with
// buildings extruded to their OSM heights. One MapLibre instance — one WebGL
// context — for the canvas's whole life. Components add sources and layers
// through `addSource`/`addLayer`, and `clear()` removes exactly those, so a
// flush can never leave a component's layers behind.
//
// Loaded on demand by canvas.js: MapLibre is ~1 MB of JavaScript, and a page
// that never shows a map never fetches it.
import * as maplibre from '../../vendor/maplibre-gl/maplibre-gl.mjs';
import { PMTiles, Protocol } from '../../vendor/pmtiles/index.js';
import { layers } from '../../vendor/basemaps/index.js';
import { ATTRIBUTION, BASEMAP, TILE_ORIGIN, basemapFlavor } from '../../map-style.js';
import { palettes } from '../../theme.js';

// Street-map archives. `cities` is the explorer's shared archive on the tile
// host. The others are small same-origin extracts for places it does not
// cover, read whole (so any static server works: no HTTP range support needed).
export const ARCHIVES = {
  cities: { url: BASEMAP },
  // browser/scripts/circuit-sample.mjs: z13-15 over the Circuit de Monaco, 660 KB.
  monaco: { url: new URL('../../data/monaco.pmtiles', import.meta.url).href, bounds: [7.405, 43.723, 7.440, 43.748], local: true },
};
const PITCH_3D = 60;
const BUILDINGS = 'surface-buildings-3d';

const protocol = new Protocol();
maplibre.addProtocol('pmtiles', protocol.tile);
const localLoads = new Map();
function loadLocal(url) {
  if (!localLoads.has(url)) {
    localLoads.set(url, fetch(url).then(async (response) => {
      if (!response.ok) throw new Error(`Cannot read ${url}: HTTP ${response.status}`);
      const bytes = await response.arrayBuffer();
      protocol.add(new PMTiles({ getKey: () => url, getBytes: async (offset, length) => ({ data: bytes.slice(offset, offset + length) }) }));
    }));
  }
  return localLoads.get(url);
}

const contains = ([w, s, e, n], [bw, bs, be, bn]) => bw >= w && be <= e && bs >= s && bn <= n;
/** The archive for `basemap` ("auto" picks a local extract covering `bounds`, else cities). */
export function pickArchive(basemap, bounds) {
  if (basemap !== 'auto') return basemap;
  return Object.entries(ARCHIVES).find(([, a]) => a.bounds && bounds && contains(a.bounds, bounds))?.[0] || 'cities';
}

function baseStyle(theme, archive) {
  const c = palettes[theme];
  return {
    version: 8,
    glyphs: `${TILE_ORIGIN}/fonts/{fontstack}/{range}.pbf`,
    sprite: `${TILE_ORIGIN}/sprites/v4/${theme}`,
    sources: { basemap: { type: 'vector', url: `pmtiles://${ARCHIVES[archive].url}`, attribution: ATTRIBUTION } },
    layers: [
      ...layers('basemap', basemapFlavor(theme), { lang: 'en' }),
      { id: BUILDINGS, type: 'fill-extrusion', source: 'basemap', 'source-layer': 'buildings', minzoom: 13,
        paint: {
          'fill-extrusion-color': c.rule,
          'fill-extrusion-height': ['coalesce', ['get', 'height'], 9],
          'fill-extrusion-base': ['coalesce', ['get', 'min_height'], 0],
          'fill-extrusion-opacity': 0.88,
        } },
    ],
  };
}

export async function createSurface(host) {
  const element = document.createElement('div');
  element.className = 'zc-map';
  const overlay = document.createElement('div');
  overlay.className = 'zc-overlay';
  host.append(element, overlay);
  let map;
  try {
    map = new maplibre.Map({ container: element, style: { version: 8, sources: {}, layers: [] }, attributionControl: false, maxPitch: 70, fadeDuration: 0 });
  } catch (error) {
    if (!/WebGL/.test(String(error?.message))) throw error;
    overlay.textContent = 'WebGL2 unavailable. This browser cannot draw the map.';
    throw error;
  }
  map.addControl(new maplibre.AttributionControl({ compact: false, customAttribution: ATTRIBUTION }), 'bottom-right');
  map.addControl(new maplibre.NavigationControl({ visualizePitch: true }), 'top-right');
  await new Promise((resolve) => map.loaded() ? resolve() : map.once('load', resolve));
  const resize = new ResizeObserver(() => map.resize());
  resize.observe(element);

  let styleKey = null;
  const sources = new Set(), ids = new Set();
  const surface = {
    kind: 'map',
    map,
    overlay,
    /** Make the base style `theme` over the archive for `basemap`; a no-op when it already is. */
    async prepare({ theme, basemap = 'auto', bounds = null }) {
      const archive = pickArchive(basemap, bounds);
      const key = `${theme}|${archive}`;
      if (key === styleKey) return archive;
      surface.clear();
      if (ARCHIVES[archive].local) await loadLocal(ARCHIVES[archive].url);
      const loaded = new Promise((resolve) => map.once('style.load', resolve));
      map.setStyle(baseStyle(theme, archive), { diff: false });
      await loaded;
      styleKey = key;
      return archive;
    },
    showBuildings(visible) { map.setLayoutProperty(BUILDINGS, 'visibility', visible ? 'visible' : 'none'); },
    /**
     * Frame `points` ([lon, lat] list) at `bearing`: fitted to the points
     * themselves in the rotated screen frame, so a rotated view is not framed
     * by the (larger) rotated bounding box. 3d then tilts about the same
     * centre, backing off a little because the near half grows.
     */
    frame(points, { view = '3d', bearing = 0, padding = 40 } = {}) {
      const pitch = view === '3d' ? PITCH_3D : 0;
      const box = element.getBoundingClientRect();
      const pad = Math.max(0, Math.min(padding, box.width / 6, box.height / 6));
      const r = bearing * Math.PI / 180, cos = Math.cos(r), sin = Math.sin(r);
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (const [lon, lat] of points) {
        const m = maplibre.MercatorCoordinate.fromLngLat([lon, lat]);
        const x = m.x * cos + m.y * sin, y = -m.x * sin + m.y * cos; // screen axes at this bearing
        x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y);
      }
      const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
      const center = new maplibre.MercatorCoordinate(cx * cos - cy * sin, cx * sin + cy * cos).toLngLat();
      const scale = Math.min((box.width - 2 * pad) / ((x1 - x0) || 1e-9), (box.height - 2 * pad) / ((y1 - y0) || 1e-9)) / 512;
      const zoom = Math.min(17, Math.log2(scale)) - (pitch ? 0.3 : 0);
      map.jumpTo({ center, zoom, bearing, pitch });
    },
    addSource(id, source) { map.addSource(id, source); sources.add(id); },
    addLayer(layer) { map.addLayer(layer); ids.add(layer.id); },
    /** Remove every source and layer a component added, and the overlay's contents. */
    clear() {
      for (const id of ids) if (map.getLayer(id)) map.removeLayer(id);
      for (const id of sources) if (map.getSource(id)) map.removeSource(id);
      ids.clear();
      sources.clear();
      overlay.replaceChildren();
    },
    owned: () => ({ layers: ids.size, sources: sources.size }),
    /** Resolves after a frame drawn once every component source is parsed and tiled. */
    async rendered() {
      const ready = () => [...sources].every((id) => map.isSourceLoaded(id));
      if (!ready()) {
        await new Promise((resolve) => {
          const check = () => { if (ready()) { map.off('sourcedata', check); resolve(); } };
          map.on('sourcedata', check);
        });
      }
      await new Promise((resolve) => { map.once('render', resolve); map.triggerRepaint(); });
    },
    /** Resolves when tiles are loaded and the camera is still. */
    idle: () => new Promise((resolve) => (map.loaded() && !map.isMoving() && map.areTilesLoaded() ? resolve() : map.once('idle', resolve))),
    destroy() { resize.disconnect(); map.remove(); element.remove(); overlay.remove(); },
  };
  return surface;
}
