// path-on-map: ordered paths on the map surface, coloured by an optional
// value per point, each with a start marker and label. Its contract is
// contract.json. `mount` adds its two sources and four layers once; `update`
// swaps the data and settings into them (setData, setPaintProperty), so a
// swap between two path-on-map views never rebuilds a layer.
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
const MARKS = `${SRC}-marks`;

/** Every point of every path, for framing and choosing a basemap. */
const points = (data) => data.groups.flatMap((g) => g.rows.path);
/** The data's [w, s, e, n]. */
export function extent(data) {
  return points(data).reduce((b, [x, y]) => [Math.min(b[0], x), Math.min(b[1], y), Math.max(b[2], x), Math.max(b[3], y)], [Infinity, Infinity, -Infinity, -Infinity]);
}

/** GeoJSON for `data` under `spec`: one feature per segment, carrying its colour; one start mark per path. */
export function features(data, spec, theme) {
  const { colorScale } = spec.options;
  const accent = palettes[theme].accent;
  const values = data.groups.flatMap((g) => g.rows.value || []);
  let colour = () => accent;
  let range = null;
  if (values.length && colorScale !== 'solid') {
    let lo = Infinity, hi = -Infinity;
    for (const v of values) { if (v < lo) lo = v; if (v > hi) hi = v; }
    const mid = colorScale === 'diverging' && lo < 0 && hi > 0 ? 0 : (lo + hi) / 2;
    const scale = (v) => colorScale === 'diverging'
      ? (v < mid ? 0.5 * (v - lo) / ((mid - lo) || 1) : 0.5 + 0.5 * (v - mid) / ((hi - mid) || 1))
      : (v - lo) / ((hi - lo) || 1);
    const ramp = RAMPS[colorScale][theme];
    colour = (v) => (v === undefined ? accent : sample(ramp, scale(v)));
    range = { lo, hi, ramp };
  }
  const segments = [], marks = [];
  data.groups.forEach((group, g) => {
    const { path, value } = group.rows;
    for (let i = 1; i < path.length; i++) {
      segments.push({ type: 'Feature', properties: { g, c: colour(value?.[i - 1]) }, geometry: { type: 'LineString', coordinates: [path[i - 1], path[i]] } });
    }
    marks.push({ type: 'Feature', properties: { g, label: group.label ?? '' }, geometry: { type: 'Point', coordinates: path[0] } });
  });
  return { segments: { type: 'FeatureCollection', features: segments }, marks: { type: 'FeatureCollection', features: marks }, range };
}

function legend(surface, range, spec) {
  surface.overlay.replaceChildren();
  if (!range) return;
  const { lo, hi, ramp } = range;
  const box = document.createElement('div');
  box.className = 'zc-legend';
  const name = document.createElement('div');
  name.className = 'zc-legend-name';
  const unit = spec.options.valueUnit;
  name.textContent = `${spec.binding.value.split('.').at(-1)}${unit ? ` (${unit})` : ''}`;
  const bar = document.createElement('div');
  bar.className = 'zc-legend-bar';
  bar.style.background = `linear-gradient(to right, ${ramp.join(', ')})`;
  const ends = document.createElement('div');
  ends.className = 'zc-legend-ends';
  const fmt = (v) => (hi - lo >= 10 ? v.toFixed(0) : String(+v.toPrecision(3)));
  const [a, b] = [document.createElement('span'), document.createElement('span')];
  a.textContent = fmt(lo);
  b.textContent = fmt(hi);
  ends.append(a, b);
  box.append(name, bar, ends);
  surface.overlay.append(box);
}

function frame(surface, data, spec) {
  const b = spec.surface.bounds;
  surface.frame(b ? [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]] : points(data), { view: spec.options.view, bearing: spec.options.bearing });
}

/** Add the sources and layers, drawing `data`. Returns the instance that later swaps update. */
export function mount(surface, { data, spec, theme }) {
  const c = palettes[theme];
  const drawn = features(data, spec, theme);
  const width = spec.options.lineWidth;
  surface.addSource(SRC, { type: 'geojson', data: drawn.segments });
  surface.addSource(MARKS, { type: 'geojson', data: drawn.marks });
  surface.addLayer({ id: `${SRC}-casing`, type: 'line', source: SRC, layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: { 'line-color': c.panel, 'line-width': width + 4, 'line-opacity': 0.95 } });
  surface.addLayer({ id: `${SRC}-line`, type: 'line', source: SRC, layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: { 'line-width': width, 'line-color': ['get', 'c'] } });
  surface.addLayer({ id: `${SRC}-start`, type: 'circle', source: MARKS,
    paint: { 'circle-radius': Math.max(6, width), 'circle-color': c.panel, 'circle-stroke-color': c.ink, 'circle-stroke-width': 3 } });
  surface.addLayer({ id: `${SRC}-label`, type: 'symbol', source: MARKS,
    layout: { 'text-field': ['get', 'label'], 'text-font': ['Noto Sans Medium'], 'text-size': 14, 'text-anchor': 'left', 'text-offset': [1.1, 0], 'text-allow-overlap': true },
    paint: { 'text-color': c.ink, 'text-halo-color': c.ground, 'text-halo-width': 2 } });
  surface.showBuildings(spec.options.showBuildings);
  legend(surface, drawn.range, spec);
  frame(surface, data, spec);

  return {
    /** Draw another path-on-map view into the same sources and layers. */
    update({ data: next, spec: nextSpec, theme: nextTheme }) {
      const map = surface.map;
      const redrawn = features(next, nextSpec, nextTheme);
      map.getSource(SRC).setData(redrawn.segments);
      map.getSource(MARKS).setData(redrawn.marks);
      const w = nextSpec.options.lineWidth;
      map.setPaintProperty(`${SRC}-casing`, 'line-width', w + 4);
      map.setPaintProperty(`${SRC}-line`, 'line-width', w);
      map.setPaintProperty(`${SRC}-start`, 'circle-radius', Math.max(6, w));
      surface.showBuildings(nextSpec.options.showBuildings);
      legend(surface, redrawn.range, nextSpec);
      frame(surface, next, nextSpec);
    },
  };
}
