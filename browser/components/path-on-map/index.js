// path-on-map: an ordered path on the map surface, coloured by an optional
// value per point, with a start/finish marker. Its contract is contract.json.
import { palettes } from '../../theme.js';

// One hue, light to dark with the value (light theme); the dark theme gets its
// own steps, dim to bright against the dark ground. Diverging: two poles
// around a neutral midpoint.
export const RAMPS = {
  sequential: {
    light: ['#F2C29C', '#E0935F', '#C46A3A', '#943F1C', '#5E2008'],
    dark: ['#6A3017', '#9A4B24', '#CC7141', '#E8A474', '#F9D8B8'],
  },
  diverging: {
    light: ['#1F6E78', '#7FA8AB', '#C9C0B2', '#D98A5C', '#9C4220'],
    dark: ['#4FA3AC', '#3F6A6D', '#6B6259', '#B8683D', '#F0A878'],
  },
};
const hex = (color) => [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
function sample(ramp, t) {
  const x = Math.max(0, Math.min(1, t)) * (ramp.length - 1);
  const i = Math.min(ramp.length - 2, Math.floor(x)), f = x - i;
  const [a, b] = [hex(ramp[i]), hex(ramp[i + 1])];
  return `rgb(${a.map((v, k) => Math.round(v + (b[k] - v) * f)).join(',')})`;
}

const SRC = 'path-on-map';
const lastField = (path) => path.replace(/\[\]/g, '').split('.').filter(Boolean).at(-1);

export function mount(surface, { data, spec, theme }) {
  const c = palettes[theme];
  const { path, value, label } = data;
  const { colorScale, lineWidth, view, bearing, showBuildings } = spec.options;

  // Distance along the path, for the gradient's stops.
  const along = [0];
  for (let i = 1; i < path.length; i++) {
    const [a, b] = [path[i - 1], path[i]];
    const k = Math.cos((a[1] + b[1]) / 2 * Math.PI / 180);
    along.push(along[i - 1] + Math.hypot((b[0] - a[0]) * k, b[1] - a[1]));
  }
  const total = along.at(-1) || 1;
  let color = c.accent;
  let range = null;
  if (value && colorScale !== 'solid') {
    let lo = Infinity, hi = -Infinity;
    for (const v of value) { if (v < lo) lo = v; if (v > hi) hi = v; }
    range = [lo, hi];
    const mid = colorScale === 'diverging' && lo < 0 && hi > 0 ? 0 : (lo + hi) / 2;
    const scale = (v) => colorScale === 'diverging'
      ? (v < mid ? 0.5 * (v - lo) / ((mid - lo) || 1) : 0.5 + 0.5 * (v - mid) / ((hi - mid) || 1))
      : (v - lo) / ((hi - lo) || 1);
    const ramp = RAMPS[colorScale][theme];
    const stops = [];
    let last = -1;
    for (let i = 0; i < path.length; i++) {
      const p = along[i] / total;
      if (p <= last) continue; // line-progress stops must increase
      stops.push(p, sample(ramp, scale(value[i])));
      last = p;
    }
    color = ['interpolate', ['linear'], ['line-progress'], ...stops];
    range.push(ramp);
  }

  surface.addSource(SRC, { type: 'geojson', lineMetrics: true, data: { type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates: path } } });
  surface.addSource(`${SRC}-start`, { type: 'geojson', data: { type: 'Feature', properties: { label: label || '' }, geometry: { type: 'Point', coordinates: path[0] } } });
  surface.addLayer({ id: `${SRC}-casing`, type: 'line', source: SRC, layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: { 'line-color': c.panel, 'line-width': lineWidth + 4, 'line-opacity': 0.95 } });
  surface.addLayer({ id: `${SRC}-line`, type: 'line', source: SRC, layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: { 'line-width': lineWidth, ...(Array.isArray(color) ? { 'line-gradient': color } : { 'line-color': color }) } });
  surface.addLayer({ id: `${SRC}-start`, type: 'circle', source: `${SRC}-start`,
    paint: { 'circle-radius': Math.max(6, lineWidth), 'circle-color': c.panel, 'circle-stroke-color': c.ink, 'circle-stroke-width': 3 } });
  if (label) {
    surface.addLayer({ id: `${SRC}-label`, type: 'symbol', source: `${SRC}-start`,
      layout: { 'text-field': ['get', 'label'], 'text-font': ['Noto Sans Medium'], 'text-size': 14, 'text-anchor': 'left', 'text-offset': [1.1, 0], 'text-allow-overlap': true },
      paint: { 'text-color': c.ink, 'text-halo-color': c.ground, 'text-halo-width': 2 } });
  }
  surface.showBuildings(showBuildings);

  // Legend: the value's name, its range and the ramp. Text in ink, never in the series colour.
  if (range) {
    const [lo, hi, ramp] = range;
    const legend = document.createElement('div');
    legend.className = 'zc-legend';
    const name = document.createElement('div');
    name.className = 'zc-legend-name';
    name.textContent = lastField(spec.binding.value);
    const bar = document.createElement('div');
    bar.className = 'zc-legend-bar';
    bar.style.background = `linear-gradient(to right, ${ramp.join(', ')})`;
    const ends = document.createElement('div');
    ends.className = 'zc-legend-ends';
    const fmt = (v) => hi - lo >= 10 ? v.toFixed(0) : String(+v.toPrecision(3));
    ends.innerHTML = '<span></span><span></span>';
    ends.children[0].textContent = fmt(lo);
    ends.children[1].textContent = fmt(hi);
    legend.append(name, bar, ends);
    surface.overlay.append(legend);
  }

  const b = spec.surface.bounds;
  surface.frame(b ? [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]] : path, { view, bearing });
}

/** The data's [w, s, e, n], for choosing a basemap before mounting. */
export function extent({ path }) {
  return path.reduce((b, [x, y]) => [Math.min(b[0], x), Math.min(b[1], y), Math.max(b[2], x), Math.max(b[3], y)], [Infinity, Infinity, -Infinity, -Infinity]);
}
