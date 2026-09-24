import { mkdir, readFile } from 'node:fs/promises';
import { test, expect } from './offline.js';
import { palettes } from '../theme.js';
import { tileFixture } from './map-fixture.js';
import { map, idle, hasColor, hex } from './globe-helpers.js';

// The Cities sample on the multi-city basemap (cities.pmtiles): eight cities,
// the non-stop routes between them, and each city's places.
const SHOTS = '../.tmp/shots';

// The sample's own files, read independently of the explorer: what every count below is checked against.
async function sampleData() {
  const rows = async (name) => {
    const [header, ...lines] = (await readFile(`samples/${name}`, 'utf8')).trim().split(/\r?\n/);
    const keys = header.split(',');
    return lines.map((line) => {
      const cells = [];
      for (const match of line.matchAll(/("(?:[^"]|"")*"|[^,]*)(,|$)/g)) {
        cells.push(match[1].startsWith('"') ? match[1].slice(1, -1).replaceAll('""', '"') : match[1]);
        if (!match[2]) break;
      }
      return Object.fromEntries(keys.map((key, i) => [key, cells[i]]));
    });
  };
  return { cities: await rows('cities-cities.csv'), places: await rows('cities-places.csv'), routes: await rows('cities-routes.csv') };
}

const editorValue = (page, pane) => page.evaluate((pane) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest(`#${pane}`)).getValue(), pane);
// A query result as a list: a query that names one node by a unique field returns that node alone.
const output = async (page) => [JSON.parse(await editorValue(page, 'output') || '[]')].flat();
const arcs = (page, script, arg) => page.evaluate(({ script, arg }) => new Function('arcs', 'arg', script)(document.querySelector('#graph')._arcs, arg), { script, arg });
const count = (page) => arcs(page, 'return arcs ? arcs.count : -1');
const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
const at = (data, name) => { const city = data.cities.find((city) => city.name === name); return [Number(city.lon), Number(city.lat)]; };

async function loadCities(page, data) {
  await mkdir(SHOTS, { recursive: true });
  await tileFixture(page);
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-cities').click();
  await expect(page.getByRole('tab', { name: 'Globe' })).toHaveAttribute('aria-selected', 'true');
  // Every route, and every place's link to its city.
  await expect.poll(() => count(page)).toBe(data.routes.length + data.places.length);
  await page.evaluate(() => document.querySelector('#graph')._outlines);
  await idle(page);
}

// The arc drawn for a route between two cities, by its ends' coordinates.
async function arcIndex(page, data, from, to) {
  const near = (a, b) => Math.abs(a[0] - b[0]) < 1e-6 && Math.abs(a[1] - b[1]) < 1e-6;
  const records = await arcs(page, 'return arcs.records.map(({ from, to }) => [from, to])');
  const index = records.findIndex(([a, b]) => near(a, at(data, from)) && near(b, at(data, to)));
  expect(index, `arc ${from} -> ${to}`).toBeGreaterThanOrEqual(0);
  return index;
}
// A point of the arc that is on the screen and drawn in the accent.
async function visiblePoint(page, index) {
  const theme = await page.locator('html').getAttribute('data-theme');
  for (const s of [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8]) {
    const point = await arcs(page, 'return arcs.screen(arg[0], arg[1])', [index, s]);
    if (point && await hasColor(page, point.x, point.y, palettes[theme].accent)) return point;
  }
  return null;
}

// The share of the canvas's pixels drawn in the basemap's street colours:
// roads and buildings (`rule`) and road casings (`strongRule`), from one
// screenshot of the map.
async function streetShare(page, theme) {
  const { rule, strongRule } = palettes[theme];
  const png = (await page.locator('.maplibregl-canvas').screenshot()).toString('base64');
  return page.evaluate(async ({ png, colors }) => {
    const image = new Image();
    image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    let street = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (colors.some((color) => color.every((channel, c) => Math.abs(data[i + c] - channel) <= 6))) street++;
    }
    return street / (data.length / 4);
  }, { png, colors: [hex(rule), hex(strongRule)] });
}

// Zoom the globe into a city until it is the flat street map.
async function zoomInto(page, lonLat, zoom = 14) {
  await map(page, 'map.jumpTo({ center: arg[0], zoom: arg[1], pitch: 0 })', [lonLat, zoom]);
  await idle(page);
}

test('the Cities sample opens on a globe with every route as an arc, and its example bar', async ({ page }) => {
  const data = await sampleData();
  expect(data.cities.map((city) => city.name)).toEqual(['Calgary', 'New York', 'San Francisco', 'London', 'Rome', 'Addis Ababa', 'Tokyo', 'Sydney']);
  await loadCities(page, data);
  const countries = new Set(data.cities.map((city) => city.country)).size;
  const places = data.cities.length + data.places.length;
  const total = data.routes.length + data.places.length;
  await expect(page.locator('.map-count')).toHaveText(`${countries} countries · ${places} places · ${data.routes.length} of ${total} relationships`);
  // Animated unless the reader prefers reduced motion.
  expect(await arcs(page, 'return arcs.animate')).toBe(true);
  // Routes on three continents are drawn in the accent, with their ends on the cities.
  for (const [from, to] of [['Calgary', 'London'], ['New York', 'London'], ['Addis Ababa', 'Rome']]) {
    const i = await arcIndex(page, data, from, to);
    expect(await visiblePoint(page, i), `an accent pixel along ${from} -> ${to}`).not.toBeNull();
    const project = (lonLat) => map(page, 'const p = map.project(arg); return { x: p.x, y: p.y }', lonLat);
    expect(distance(await arcs(page, 'return arcs.screen(arg, 0)', i), await project(at(data, from)))).toBeLessThan(2);
    expect(distance(await arcs(page, 'return arcs.screen(arg, 1)', i), await project(at(data, to)))).toBeLessThan(2);
  }
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('OpenStreetMap');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('OpenFlights');
  await expect(page.locator('#tour-queries button')).toHaveText(['Every route', ...data.cities.map((city) => city.name)]);
  await expect(page.locator('#tour-queries button.active')).toHaveText('Every route');
});

test('the example queries return the counts in the sample files', async ({ page }) => {
  const data = await sampleData();
  await loadCities(page, data);
  // Every route: each city with the routes stored from it.
  const every = await output(page);
  expect(every.map((city) => city.name).sort()).toEqual(data.cities.map((city) => city.name).sort());
  expect(every.flatMap((city) => (city.route || []).map((to) => `${city.name}->${to.name}`)).sort())
    .toEqual(data.routes.map((route) => `${route.from}->${route.to}`).sort());
  // Each city: the city and exactly its own places.
  for (const city of data.cities) {
    await page.locator('#tour-queries button', { hasText: new RegExp(`^${city.name}$`) }).click();
    await expect(page.locator('#tour-queries button.active')).toHaveText(city.name);
    await expect.poll(async () => (await output(page))[0]?.name).toBe(city.name);
    const result = await output(page);
    expect(result).toHaveLength(1);
    const expected = data.places.filter((place) => place.city === city.name);
    expect(expected.length).toBe(12);
    expect(result[0].places.map((place) => place.name).sort()).toEqual(expected.map((place) => place.name).sort());
  }
});

for (const [name, tile] of [['London', [-0.12085, 51.51558]], ['Tokyo', [139.73511, 35.66622]]]) {
  test(`zooming the globe into ${name} shows its streets and its places`, async ({ page }) => {
    const data = await sampleData();
    await loadCities(page, data);
    // On the globe, before zooming in, the basemap is not drawn at all.
    expect(await streetShare(page, 'light')).toBeLessThan(0.01);
    await zoomInto(page, tile);
    expect(await map(page, 'return map.style.projection.transitionState')).toBe(0);
    // Streets: the archive's roads are rendered, and they are on the screen.
    const roads = await map(page, 'return map.queryRenderedFeatures({ layers: map.getStyle().layers.filter((l) => /^roads_/.test(l.id)).map((l) => l.id) }).length');
    expect(roads, 'rendered road features').toBeGreaterThan(50);
    expect(await streetShare(page, 'light'), 'street-coloured pixels').toBeGreaterThan(0.05);
    // The city's places are drawn over them.
    const ids = await map(page, 'return map.queryRenderedFeatures({ layers: ["zega-nodes"] }).map((f) => f.properties.id)');
    expect(ids.length, `${name} places on screen`).toBeGreaterThan(0);
    await expect(page.locator('.map-notice')).toBeHidden();
  });
}

test('each city\'s query frames the flat map on that city\'s places', async ({ page }) => {
  const data = await sampleData();
  await loadCities(page, data);
  for (const name of ['Addis Ababa', 'Sydney']) {
    await page.locator('#tour-queries button', { hasText: new RegExp(`^${name}$`) }).click();
    await expect.poll(async () => (await output(page))[0]?.name).toBe(name);
    await page.getByRole('tab', { name: 'Map' }).click();
    await expect(page.locator('.map-count')).toHaveText('12 places');
    await expect.poll(() => map(page, 'return !!map && map.loaded()')).toBe(true);
    const bounds = await map(page, 'const b = map.getBounds(); return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()]');
    // Every one of the city's places is in the frame.
    for (const place of data.places.filter((place) => place.city === name)) {
      const [lon, lat] = [Number(place.lon), Number(place.lat)];
      expect(lon > bounds[0] && lon < bounds[2] && lat > bounds[1] && lat < bounds[3], `${place.name} in the frame`).toBe(true);
    }
    // A city's places, not the world: the frame is a few kilometres across.
    expect(bounds[2] - bounds[0]).toBeLessThan(0.2);
    await page.getByRole('tab', { name: 'Globe' }).click();
  }
});
