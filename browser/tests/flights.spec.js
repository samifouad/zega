import { mkdir, readFile } from 'node:fs/promises';
import { test, expect } from './offline.js';
import { palettes } from '../theme.js';
import { tileFixture } from './map-fixture.js';
import { map, idle, hasColor } from './globe-helpers.js';

// zega#83: the Flights sample, its example bar, and the #81 follow-ups.
const SHOTS = '../.tmp/shots';
const TOWER = [51.0443, -114.0631];

// The sample's own files, read independently of the explorer: what every count below is checked against.
async function sampleData() {
  const rows = async (name) => {
    const [header, ...lines] = (await readFile(`samples/${name}`, 'utf8')).trim().split(/\r?\n/);
    const keys = header.split(',');
    return lines.map((line) => Object.fromEntries(line.split('","').map((cell, i) => [keys[i], cell.replaceAll('"', '')])));
  };
  const airports = await rows('flights-airports.csv');
  const routes = await rows('flights-routes.csv');
  const countries = await rows('flights-countries.csv');
  return { airports, routes, countries };
}
const radians = (x) => x * Math.PI / 180;
function haversine([lat1, lon1], [lat2, lon2]) {
  const h = Math.sin(radians(lat2 - lat1) / 2) ** 2 + Math.cos(radians(lat1)) * Math.cos(radians(lat2)) * Math.sin(radians(lon2 - lon1) / 2) ** 2;
  return 2 * 6371008.8 * Math.atan2(Math.sqrt(h), Math.sqrt(1 - h));
}

const editorValue = (page, pane) => page.evaluate((pane) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest(`#${pane}`)).getValue(), pane);
const output = async (page) => JSON.parse(await editorValue(page, 'output'));
const arcs = (page, script, arg) => page.evaluate(({ script, arg }) => new Function('arcs', 'arg', script)(document.querySelector('#graph')._arcs, arg), { script, arg });
const count = (page) => arcs(page, 'return arcs ? arcs.count : -1');
const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);

async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
}
async function loadFlights(page, { routes }) {
  await mkdir(SHOTS, { recursive: true });
  await tileFixture(page);
  await ready(page);
  await page.locator('#btn-flights').click();
  await expect(page.getByRole('tab', { name: 'Globe' })).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => count(page)).toBe(routes.length);
  await page.evaluate(() => document.querySelector('#graph')._outlines);
  await idle(page);
}
// The arc drawn for a route, by its ends' coordinates.
async function arcIndex(page, data, origin, destination) {
  const at = (code) => { const a = data.airports.find((airport) => airport.code === code); return [Number(a.lon), Number(a.lat)]; };
  const near = (a, b) => Math.abs(a[0] - b[0]) < 1e-3 && Math.abs(a[1] - b[1]) < 1e-3;
  const records = await arcs(page, 'return arcs.records.map(({ from, to }) => [from, to])');
  const index = records.findIndex(([from, to]) => near(from, at(origin)) && near(to, at(destination)));
  expect(index, `arc ${origin} -> ${destination}`).toBeGreaterThanOrEqual(0);
  return index;
}
// A point of the arc that is on the screen and in front of the globe: seen from the sample's camera, which is what the sample is for.
async function visiblePoint(page, index) {
  for (const s of [0.5, 0.4, 0.6, 0.3, 0.7]) {
    const point = await arcs(page, 'return arcs.screen(arg[0], arg[1])', [index, s]);
    if (point && await hasColor(page, point.x, point.y, palettes[await page.locator('html').getAttribute('data-theme')].accent)) return { point, s };
  }
  return null;
}

test('the Flights sample loads onto a tilted globe that draws every route as an arc, with its credit', async ({ page }) => {
  const data = await sampleData();
  expect(data.routes.length).toBeGreaterThan(500);
  await loadFlights(page, data);
  await expect(page.locator('.map-count')).toHaveText(`${data.airports.length} places · ${data.routes.length} relationships`);
  expect(await map(page, 'return [map.getPitch(), map.getZoom()]')).toEqual([40, 1]);
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('OpenFlights');
  await expect(page.locator('.maplibregl-ctrl-attrib a[href="https://openflights.org/data.php"]')).toHaveCount(1);
  // The route Calgary–Amsterdam is drawn in the accent, on the screen, and its ends sit on the two airports.
  const i = await arcIndex(page, data, 'YYC', 'AMS');
  const seen = await visiblePoint(page, i);
  expect(seen, 'an accent-coloured pixel along YYC-AMS').not.toBeNull();
  const at = (code) => { const a = data.airports.find((airport) => airport.code === code); return [Number(a.lon), Number(a.lat)]; };
  const project = (lonLat) => map(page, 'const p = map.project(arg); return { x: p.x, y: p.y }', lonLat);
  expect(distance(await arcs(page, 'return arcs.screen(arg, 0)', i), await project(at('YYC')))).toBeLessThan(2);
  expect(distance(await arcs(page, 'return arcs.screen(arg, 1)', i), await project(at('AMS')))).toBeLessThan(2);
  // The whole canvas is not one colour: the arcs are drawn, not just counted.
  const shot = await page.locator('.maplibregl-canvas').screenshot();
  const png = shot.toString('base64');
  const distinct = await page.evaluate(async (png) => {
    const image = new Image();
    image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    const colours = new Set();
    for (let i = 0; i < data.length; i += 4) colours.add((data[i] << 16) | (data[i + 1] << 8) | data[i + 2]);
    return colours.size;
  }, png);
  expect(distinct).toBeGreaterThan(100);
  // The example bar shows the sample's queries and survives a reload, as does the credit.
  await expect(page.locator('#tour')).toBeVisible();
  await expect(page.locator('#tour-queries button')).toHaveText(['Out of Calgary', 'Into Amsterdam', "Canada's airports", 'Nearest to the Calgary Tower']);
  await expect(page.locator('#tour-queries button.active')).toHaveText('Out of Calgary');
  await page.reload();
  await expect(page.getByRole('tab', { name: 'Globe' })).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => count(page)).toBe(data.routes.length);
  await expect(page.locator('#tour-queries button')).toHaveCount(4);
  await expect(page.locator('#tour-queries button.active')).toHaveText('Out of Calgary');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('OpenFlights');
  // Clear takes the sample's bar and credit with the data.
  await page.locator('#btn-clear').click();
  await expect(page.locator('#tour')).toBeHidden();
  expect(await page.evaluate(() => localStorage.getItem('zega.v2.sample'))).toBeNull();
});

test('the example queries return the counts in the sample files, and the globe stays put between them', async ({ page }) => {
  const data = await sampleData();
  await loadFlights(page, data);
  const buttons = page.locator('#tour-queries button');
  const codesOf = (list) => list.map((row) => row.code).sort();
  const shot = async (n) => page.screenshot({ path: `${SHOTS}/flights-query-${n}-1440.png` });
  // 1. Out of Calgary: every route stored from YYC.
  await buttons.nth(0).click();
  await expect.poll(async () => (await output(page)).route?.length).toBe(data.routes.filter((r) => r.origin === 'YYC').length);
  const yyc = await output(page);
  expect(yyc.name).toBe('Calgary International Airport');
  expect(codesOf(yyc.route)).toEqual(data.routes.filter((r) => r.origin === 'YYC').map((r) => r.destination).sort());
  await shot(1);
  const created = await map(page, 'return map._zegaStamp = map._zegaStamp || Math.random()');
  // 2. Into Amsterdam: every route stored to AMS, each with its country.
  await buttons.nth(1).click();
  await expect.poll(async () => (await output(page)).inbound?.length).toBe(data.routes.filter((r) => r.destination === 'AMS').length);
  const ams = await output(page);
  expect(codesOf(ams.inbound)).toEqual(data.routes.filter((r) => r.destination === 'AMS').map((r) => r.origin).sort());
  for (const row of ams.inbound) {
    const airport = data.airports.find((a) => a.code === row.code);
    expect(row.country.name).toBe(data.countries.find((c) => c.iso === airport.country).name);
  }
  await shot(2);
  // 3. Canada's airports, each with the routes stored from it.
  await buttons.nth(2).click();
  const canada = data.airports.filter((a) => a.country === 'CA');
  await expect.poll(async () => (await output(page)).airports?.length).toBe(canada.length);
  const ca = await output(page);
  expect(ca.name).toBe('Canada');
  expect(codesOf(ca.airports)).toEqual(codesOf(canada));
  for (const airport of ca.airports) expect(airport.route.length, airport.code).toBe(data.routes.filter((r) => r.origin === airport.code).length);
  await shot(3);
  // 4. The five airports nearest the Calgary Tower, nearest first, with the haversine distance.
  await buttons.nth(3).click();
  const nearest = [...data.airports].map((a) => ({ code: a.code, distance: haversine(TOWER, [Number(a.lat), Number(a.lon)]) })).sort((a, b) => a.distance - b.distance).slice(0, 5);
  await expect.poll(async () => (await output(page)).length).toBe(5);
  const five = await output(page);
  expect(five.map((row) => row.code)).toEqual(nearest.map((row) => row.code));
  five.forEach((row, i) => expect(row.distance).toBeCloseTo(nearest[i].distance, 0));
  await shot(4);
  // The same globe, throughout: the queries changed the output pane, not the view.
  expect(await map(page, 'return map._zegaStamp')).toBe(created);
  expect(await count(page)).toBe(data.routes.length);
});

test('shaders are freed with their programs: the live count does not grow across view switches', async ({ page }) => {
  await page.addInitScript(() => {
    const counts = { created: 0, deleted: 0 };
    window.__shaders = counts;
    const proto = WebGL2RenderingContext.prototype;
    const create = proto.createShader, remove = proto.deleteShader;
    proto.createShader = function (...args) { counts.created++; return create.apply(this, args); };
    proto.deleteShader = function (...args) { counts.deleted++; return remove.apply(this, args); };
  });
  const data = await sampleData();
  await loadFlights(page, data);
  const live = () => page.evaluate(() => window.__shaders.created - window.__shaders.deleted);
  expect(await page.evaluate(() => window.__shaders.created)).toBeGreaterThan(0);
  const first = await live();
  for (let cycle = 1; cycle <= 3; cycle++) {
    await page.getByRole('tab', { name: 'Table' }).click();
    await expect(page.locator('.table-view')).toBeVisible();
    await page.getByRole('tab', { name: 'Globe' }).click();
    await expect.poll(() => count(page)).toBe(data.routes.length);
    await idle(page);
    expect(await live(), `after ${cycle} switches`).toBe(first);
  }
});

test('without WebGL2 the globe and the map say so in the view, and the query still answers', async ({ page }) => {
  await page.addInitScript(() => {
    const get = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type, ...rest) {
      return type === 'webgl2' || type === 'webgl' ? null : get.call(this, type, ...rest);
    };
  });
  const data = await sampleData();
  await mkdir(SHOTS, { recursive: true });
  await ready(page);
  await page.locator('#btn-flights').click();
  await expect(page.getByRole('tab', { name: 'Globe' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.map-notice')).toHaveText('WebGL2 unavailable. This browser cannot draw the globe.');
  await expect.poll(async () => (await output(page)).route?.length).toBe(data.routes.filter((r) => r.origin === 'YYC').length);
  await expect(page.locator('#raw-count')).toHaveText(`${data.airports.length + data.countries.length} nodes · ${data.routes.length + data.airports.length} edges`);
  await page.screenshot({ path: `${SHOTS}/flights-no-webgl2-1440.png` });
  await page.getByRole('tab', { name: 'Map', exact: true }).click();
  await expect(page.locator('.map-notice')).toHaveText('WebGL2 unavailable. This browser cannot draw the map.');
  await expect.poll(async () => (await output(page)).route?.length).toBe(data.routes.filter((r) => r.origin === 'YYC').length);
});

for (const width of [1440, 390]) test(`the Flights globe in light and dark at ${width}px, with the animation moving`, async ({ page }) => {
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  const data = await sampleData();
  await loadFlights(page, data);
  for (const theme of ['light', 'dark']) {
    if (await page.locator('html').getAttribute('data-theme') !== theme) {
      await page.locator('#btn-theme').click();
      await expect.poll(() => map(page, 'return !!map && map.loaded() && !!map.getLayer("globe-land")')).toBe(true);
      await page.evaluate(() => document.querySelector('#graph')._outlines);
      await expect.poll(() => count(page)).toBe(data.routes.length);
      await idle(page);
    }
    await page.evaluate(() => window.scrollTo(0, 0));
    const i = await arcIndex(page, data, 'YYC', 'AMS');
    expect(await visiblePoint(page, i), `${theme} ${width}`).not.toBeNull();
    await page.screenshot({ path: `${SHOTS}/flights-globe-${theme}-${width}.png` });
  }
  const frames = [];
  for (let n = 1; n <= 3; n++) {
    frames.push(await page.locator('.maplibregl-canvas').screenshot({ path: `${SHOTS}/flights-frame-${n}-${width}.png` }));
    await page.waitForTimeout(150);
  }
  expect(frames[0].equals(frames[1])).toBe(false);
  expect(frames[1].equals(frames[2])).toBe(false);
});
