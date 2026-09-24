import { mkdir } from 'node:fs/promises';
import { test, expect } from './offline.js';
import { palettes } from '../theme.js';
import { globe, map, globeness, idle, hasColor } from './globe-helpers.js';

// zega#74: relationships as animated arcs lifted off the globe.
const SCHEMA = `schema {
  type Country { name: String iso: String<iso2> }
  type City { name: String at: Point flight -> City[] visit -> Country[] }
  display {
    globe(@zoom: 1.8, @tilt: 0, @center: @point(45, -30)) { Country, City } : Default
    table
  }
}
unique { City { name } Country { name } }
mutation { Country(name: "Canada" && iso: "CA") { name } }
mutation { Country(name: "Japan" && iso: "JP") { name } }
mutation { Country(name: "Brazil" && iso: "BR") { name } }
mutation { City(name: "Calgary" && at: @point(51.05, -114.07)) { name } }
mutation { City(name: "Lisbon" && at: @point(38.72, -9.14)) { name } }
mutation { City(name: "Tokyo" && at: @point(35.68, 139.69)) { name } }
mutation { City(name: "Honolulu" && at: @point(21.31, -157.86)) { name } }
mutation { City(name: "Airport" && at: @point(51.13, -114.01)) { name } }
mutation { City(name: "Dateline West" && at: @point(-16.5, 179.9)) { name } }
mutation { City(name: "Dateline East" && at: @point(-16.5, -179.9)) { name } }
mutation { City(name: "Calgary") { flight -> link City(name: "Lisbon") } }
mutation { City(name: "Tokyo") { flight -> link City(name: "Honolulu") } }
mutation { City(name: "Calgary") { flight -> link City(name: "Airport") } }
mutation { City(name: "Dateline West") { flight -> link City(name: "Dateline East") } }
mutation { City(name: "Lisbon") { visit -> link Country(name: "Brazil") } }`;
const ROUTES = 5;
const CALGARY = [-114.07, 51.05], LISBON = [-9.14, 38.72], TOKYO = [139.69, 35.68], HONOLULU = [-157.86, 21.31], AIRPORT = [-114.01, 51.13];
const WEST = [179.9, -16.5], EAST = [-179.9, -16.5];
const SHOTS = '../.tmp/shots';

// Great-circle midpoint, independently of the layer: the normalised sum of the unit vectors.
const vec = ([lon, lat]) => { const p = lat * Math.PI / 180, l = lon * Math.PI / 180; return [Math.sin(l) * Math.cos(p), Math.sin(p), Math.cos(l) * Math.cos(p)]; };
function geoMid(a, b) {
  const p = vec(a), q = vec(b);
  const m = p.map((x, i) => x + q[i]), n = Math.hypot(...m);
  return [Math.atan2(m[0] / n, m[2] / n) * 180 / Math.PI, Math.asin(m[1] / n) * 180 / Math.PI];
}
// The point a fraction `s` along the great circle from a to b.
function along(a, b, s) {
  const p = vec(a), q = vec(b);
  const omega = Math.acos(Math.max(-1, Math.min(1, p[0] * q[0] + p[1] * q[1] + p[2] * q[2])));
  const m = p.map((x, i) => Math.sin((1 - s) * omega) / Math.sin(omega) * x + Math.sin(s * omega) / Math.sin(omega) * q[i]);
  return [Math.atan2(m[0], m[2]) * 180 / Math.PI, Math.asin(m[1]) * 180 / Math.PI];
}
const antipode = ([lon, lat]) => [lon > 0 ? lon - 180 : lon + 180, -lat];
const distance = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);

const arcs = (page, script, arg) => page.evaluate(({ script, arg }) => new Function('arcs', 'arg', script)(document.querySelector('#graph')._arcs, arg), { script, arg });
const count = (page) => arcs(page, 'return arcs ? arcs.count : -1');
const screen = (page, index, s) => arcs(page, 'return arcs.screen(arg[0], arg[1])', [index, s]);
const project = (page, lonLat) => map(page, 'const p = map.project(arg); return { x: p.x, y: p.y }', lonLat);
async function arcIndex(page, from, to) {
  const near = (a, b) => Math.abs(a[0] - b[0]) < 1e-6 && Math.abs(a[1] - b[1]) < 1e-6;
  const records = await arcs(page, 'return arcs.records.map(({ from, to }) => [from, to])');
  const index = records.findIndex(([a, b]) => near(a, from) && near(b, to));
  expect(index, `arc ${from} -> ${to}`).toBeGreaterThanOrEqual(0);
  return index;
}
async function look(page, center, extra = {}) {
  await map(page, 'map.jumpTo({ center: arg.center, ...arg.extra })', { center, extra });
  await idle(page);
}
async function open(page, schema = SCHEMA) {
  await mkdir(SHOTS, { recursive: true });
  await globe(page, schema);
  await expect.poll(() => count(page)).toBe(ROUTES);
  await idle(page);
}
const settings = (page) => page.locator('.globe-settings');
async function openSettings(page) {
  if (!await settings(page).isVisible()) await page.locator('.globe-gear').click();
  await expect(settings(page)).toBeVisible();
}
async function setLift(page, value) {
  await openSettings(page);
  await page.locator('input[data-p="lift"]').evaluate((input, value) => { input.value = value; input.dispatchEvent(new Event('input', { bubbles: true })); }, String(value));
  await idle(page);
}
const canvasShot = (page) => page.locator('.maplibregl-canvas').screenshot();
const light = palettes.light.accent;

test('an arc lifts off the surface at its midpoint, hides behind the globe and reappears after rotating', async ({ page }) => {
  await open(page);
  await expect(page.locator('.map-count')).toHaveText(`3 countries · 7 places · ${ROUTES} relationships`);
  const i = await arcIndex(page, CALGARY, LISBON);
  const mid = geoMid(CALGARY, LISBON);
  // Seen from 50° aside, the lift is a screen offset outward from the surface point.
  await look(page, [mid[0] + 50, mid[1]]);
  const lifted = await screen(page, i, 0.5), surface = await project(page, mid);
  expect(distance(lifted, surface)).toBeGreaterThan(12);
  expect(await hasColor(page, lifted.x, lifted.y, light)).toBe(true);
  expect(await hasColor(page, surface.x, surface.y, light, { r: 3 })).toBe(false);
  // The ends sit on their nodes: the layer's projection agrees with MapLibre's.
  expect(distance(await screen(page, i, 0), await project(page, CALGARY))).toBeLessThan(2);
  expect(distance(await screen(page, i, 1), await project(page, LISBON))).toBeLessThan(2);
  // From the antipode both ends are on the far side: the arc is behind the planet.
  await look(page, antipode(mid));
  const hidden = await screen(page, i, 0.5);
  expect(hidden).not.toBeNull();
  expect(await hasColor(page, hidden.x, hidden.y, light)).toBe(false);
  // Rotating back brings it over the limb.
  await look(page, mid);
  const back = await screen(page, i, 0.5);
  expect(await hasColor(page, back.x, back.y, light)).toBe(true);
  await page.screenshot({ path: `${SHOTS}/arcs-globe-1440.png` });
});

test('an arc across the antimeridian follows the great circle, on the globe and on the flat map', async ({ page }) => {
  await open(page);
  const i = await arcIndex(page, TOKYO, HONOLULU);
  const mid = geoMid(TOKYO, HONOLULU);
  expect(Math.abs(mid[0])).toBeGreaterThan(170); // the short way crosses 180°
  await look(page, mid, { zoom: 1.8 });
  // Face on, the lift is towards the camera: the arc passes over the surface midpoint.
  const surface = await project(page, mid);
  expect(await hasColor(page, surface.x, surface.y, light)).toBe(true);
  // ... and it is one continuous line either side of the antimeridian.
  let previous = null;
  for (const s of [0.3, 0.4, 0.5, 0.6, 0.7]) {
    const point = await screen(page, i, s);
    expect(await hasColor(page, point.x, point.y, light), `s=${s}`).toBe(true);
    if (previous) expect(distance(point, previous)).toBeLessThan(120);
    previous = point;
  }
  await page.screenshot({ path: `${SHOTS}/arcs-antimeridian-1440.png` });
  // A 21 km hop across the dateline, mid-blend and on the flat map: drawn on
  // the side the reader looks at. Its apex (7.6 km) is above the flat map's
  // camera, so the flat check looks straight down at a lower point.
  const j = await arcIndex(page, WEST, EAST);
  await look(page, [180, -16.5], { zoom: 11.5 });
  expect(await globeness(page)).toBeLessThan(1);
  const apex = await screen(page, j, 0.5);
  expect(await hasColor(page, apex.x, apex.y, light)).toBe(true);
  await look(page, along(WEST, EAST, 0.05), { zoom: 12.2 });
  expect(await globeness(page)).toBe(0);
  const flat = await screen(page, j, 0.05);
  expect(await hasColor(page, flat.x, flat.y, light)).toBe(true);
  await look(page, along(EAST, WEST, 0.05), { zoom: 12.2 });
  const other = await screen(page, j, 0.95);
  expect(await hasColor(page, other.x, other.y, light)).toBe(true);
});

test('arcs blend into the flat map with the globe, without a jump', async ({ page }) => {
  await open(page);
  const i = await arcIndex(page, CALGARY, AIRPORT);
  const mid = geoMid(CALGARY, AIRPORT);
  // MapLibre's `globe` preset is the sphere to zoom 11 and the flat map from 12.
  await look(page, mid, { zoom: 11.5 });
  expect(await globeness(page)).toBeGreaterThan(0);
  expect(await globeness(page)).toBeLessThan(1);
  // The dashes move, so one frame can put a gap on the sampled point: poll a
  // few frames. A missing arc still fails.
  const blended = await screen(page, i, 0.4);
  await expect.poll(() => hasColor(page, blended.x, blended.y, light)).toBe(true);
  await page.screenshot({ path: `${SHOTS}/arcs-mid-transition-1440.png` });
  await look(page, mid, { zoom: 12.5 });
  expect(await globeness(page)).toBe(0);
  const flat = await screen(page, i, 0.45);
  await expect.poll(() => hasColor(page, flat.x, flat.y, light)).toBe(true);
  await page.screenshot({ path: `${SHOTS}/arcs-flat-1440.png` });
  // Tilted, the apex's height shows as a screen offset above the midpoint.
  // Across the end of the blend it scales with zoom alone: no jump in height.
  const rise = async (zoom) => {
    await look(page, mid, { zoom, pitch: 60 });
    return distance(await screen(page, i, 0.5), await project(page, mid));
  };
  const ratio = await rise(11.9) / await rise(12.1);
  expect(ratio).toBeGreaterThan(0.75);
  expect(ratio).toBeLessThan(1);
  await look(page, mid, { zoom: 11.5, pitch: 45 });
  await page.screenshot({ path: `${SHOTS}/arcs-mid-transition-tilted-1440.png` });
});

test('clicking an arc opens the relationship in the inspector', async ({ page }) => {
  await open(page);
  const i = await arcIndex(page, CALGARY, LISBON);
  const mid = geoMid(CALGARY, LISBON);
  await look(page, [mid[0] + 50, mid[1]]);
  const point = await screen(page, i, 0.5);
  await page.locator('.maplibregl-canvas').click({ position: point });
  await expect(page.locator('#node-inspector h3')).toHaveText('flight');
  await expect(page.locator('#node-inspector')).toContainText('Calgary');
  await expect(page.locator('#node-inspector')).toContainText('Lisbon');
  await page.locator('#node-inspector button').click();
  // A country arc ends at the country's label point, inside the country.
  const j = await arcIndex(page, LISBON, [-49.03, -11.81]);
  await look(page, [-49.03, -11.81]);
  expect(distance(await screen(page, j, 1), await project(page, [-49.03, -11.81]))).toBeLessThan(2);
});

test.describe('reduced motion', () => {
  test.use({ reducedMotion: 'reduce' });
  test('makes the arcs static by default; the edges setting animates them', async ({ page }) => {
    await open(page);
    await openSettings(page);
    await expect(page.locator('select[data-p="edges"]')).toHaveValue('static');
    const frames = () => arcs(page, 'return arcs.frames');
    const before = await frames(), still = await canvasShot(page);
    await page.waitForTimeout(400);
    expect(await frames()).toBe(before);
    expect((await canvasShot(page)).equals(still)).toBe(true);
    await page.locator('select[data-p="edges"]').selectOption('animated');
    await page.waitForTimeout(200);
    const moving = await frames(), first = await canvasShot(page);
    await page.waitForTimeout(400);
    expect(await frames()).toBeGreaterThan(moving);
    expect((await canvasShot(page)).equals(first)).toBe(false);
  });
});

test('the lift setting moves the arc midpoint and persists', async ({ page }) => {
  await open(page);
  const i = await arcIndex(page, CALGARY, LISBON);
  const mid = geoMid(CALGARY, LISBON);
  await look(page, [mid[0] + 50, mid[1]]);
  const low = await screen(page, i, 0.5);
  await setLift(page, 0.4);
  const high = await screen(page, i, 0.5);
  expect(distance(low, high)).toBeGreaterThan(10);
  expect(await hasColor(page, high.x, high.y, light)).toBe(true);
  await page.screenshot({ path: `${SHOTS}/arcs-settings-1440.png` });
  await setLift(page, 0);
  expect(distance(await screen(page, i, 0.5), await project(page, mid))).toBeLessThan(2);
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem('zega.browser.globe'))).lift).toBe(0);
});

test('auto-spin rotates the camera westward until switched off', async ({ page }) => {
  await open(page);
  await openSettings(page);
  const lng = () => map(page, 'return map.getCenter().lng');
  const start = await lng();
  await page.locator('input[data-p="spin"]').check();
  await page.waitForTimeout(600);
  const spun = await lng();
  expect(start - spun).toBeGreaterThan(0.5);
  await page.locator('input[data-p="spin"]').uncheck();
  await page.waitForTimeout(100);
  const stopped = await lng();
  await page.waitForTimeout(300);
  expect(Math.abs(await lng() - stopped)).toBeLessThan(1e-6);
});

for (const width of [1440, 390]) test(`arcs take the theme's accent in light and dark at ${width}px, and the animation moves`, async ({ page }) => {
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  await open(page);
  for (const theme of ['light', 'dark']) {
    if (await page.locator('html').getAttribute('data-theme') !== theme) {
      await page.locator('#btn-theme').click();
      await expect.poll(() => map(page, 'return !!map && map.loaded() && !!map.getLayer("globe-countries")')).toBe(true);
      await page.evaluate(() => document.querySelector('#graph')._outlines);
      await expect.poll(() => count(page)).toBe(ROUTES);
      await idle(page);
    }
    const i = await arcIndex(page, CALGARY, LISBON);
    const mid = geoMid(CALGARY, LISBON);
    // The whole globe fits the pane at either width.
    await look(page, [mid[0] + 50, mid[1]], { zoom: width === 390 ? 0.6 : 1.8 });
    await page.evaluate(() => window.scrollTo(0, 0));
    const point = await screen(page, i, 0.5);
    expect(await hasColor(page, point.x, point.y, palettes[theme].accent), theme).toBe(true);
    await page.screenshot({ path: `${SHOTS}/arcs-${theme}-${width}.png` });
    if (width === 1440) {
      await openSettings(page);
      await page.screenshot({ path: `${SHOTS}/arcs-settings-${theme}-${width}.png` });
      await page.locator('.globe-gear').click();
    }
  }
  const frames = [];
  for (let n = 1; n <= 3; n++) {
    frames.push(await canvasShot(page));
    await page.locator('.maplibregl-canvas').screenshot({ path: `${SHOTS}/arcs-frame-${n}-${width}.png` });
    await page.waitForTimeout(150);
  }
  expect(frames[0].equals(frames[1])).toBe(false);
  expect(frames[1].equals(frames[2])).toBe(false);
});
