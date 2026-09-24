import { test, expect } from './offline.js';
import { palettes } from '../theme.js';
import { globe as openGlobe, map, globeness, idle, highlightAt, setEditor } from './globe-helpers.js';

const SCHEMA = `schema {
  type Country { name: String iso: String<iso2> }
  type City { name: String at: Point }
  display {
    globe(@zoom: 2, @tilt: 20, @center: @point(50, -60)) { Country, City } : Default
    table
  }
}
mutation { Country(name: "Canada" && iso: "CA") { name } }
mutation { Country(name: "Japan" && iso: "JP") { name } }
mutation { Country(name: "Brazil" && iso: "BR") { name } }
mutation { City(name: "Calgary" && at: @point(51.05, -114.07)) { name } }
mutation { City(name: "Lisbon" && at: @point(38.72, -9.14)) { name } }`;

const CANADA = [-100, 58], USA = [-100, 40], JAPAN = [138.5, 36.5];

const globe = (page, schema = SCHEMA) => openGlobe(page, schema);

test('globe renders a sphere at the checked camera and highlights countries by ISO code', async ({ page }) => {
  await globe(page);
  await expect(page.getByRole('tab')).toHaveText(['Globe', 'Table']);
  await expect(page.locator('.map-count')).toHaveText('3 countries · 2 places');
  expect(await globeness(page)).toBe(1);
  expect(await map(page, 'const c = map.getCenter(); return [map.getZoom(), map.getPitch(), +c.lat.toFixed(4), +c.lng.toFixed(4)]')).toEqual([2, 20, 50, -60]);
  // The engine checked String<iso2>; the outlines carry the same codes.
  await expect.poll(() => highlightAt(page, ...CANADA)).toBe('CA');
  await expect.poll(() => highlightAt(page, ...USA)).toBe(null);
  await map(page, 'map.jumpTo({ center: [138, 36] })');
  await idle(page);
  await expect.poll(() => highlightAt(page, ...JAPAN)).toBe('JP');
  await map(page, 'map.jumpTo({ center: [-55, -10] })');
  await idle(page);
  await expect.poll(() => highlightAt(page, -55, -10)).toBe('BR');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('© OpenStreetMap contributors');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('Natural Earth');
});

test('clicking a highlighted country or a place opens the node inspector; other countries do nothing', async ({ page }) => {
  await globe(page);
  const click = async (lon, lat) => {
    const point = await map(page, 'return map.project(arg)', [lon, lat]);
    await page.locator('.maplibregl-canvas').click({ position: point });
  };
  await click(-100, 58);
  await expect(page.locator('#node-inspector')).toContainText('Canada');
  await expect(page.locator('#node-inspector')).toContainText('CA');
  await page.locator('#node-inspector button').click();
  await click(-114.07, 51.05);
  await expect(page.locator('#node-inspector h3')).toContainText('Calgary');
  await page.locator('#node-inspector button').click();
  await click(-100, 40); // the United States: an outline with no node
  await page.waitForTimeout(300);
  await expect(page.locator('#node-inspector')).toHaveCount(0);
});

test('zooming in turns the globe into the flat map, and back', async ({ page }) => {
  await globe(page);
  expect(await globeness(page)).toBe(1);
  await map(page, 'map.jumpTo({ center: [-114.07, 51.05], zoom: 13 })');
  await idle(page);
  await expect.poll(() => globeness(page)).toBe(0);
  await expect.poll(() => map(page, 'return map.queryRenderedFeatures({ layers: ["zega-nodes"] }).length')).toBe(1);
  await map(page, 'map.jumpTo({ zoom: 2 })');
  await idle(page);
  await expect.poll(() => globeness(page)).toBe(1);
});

for (const width of [1440, 390]) test(`globe follows the theme in light and dark and fits ${width}px`, async ({ page }) => {
  {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await globe(page);
    for (const theme of ['light', 'dark']) {
      if (await page.locator('html').getAttribute('data-theme') !== theme) {
        await page.locator('#btn-theme').click();
        await expect.poll(() => map(page, 'return !!map && map.loaded() && !!map.getLayer("globe-countries")')).toBe(true);
        await page.evaluate(() => document.querySelector('#graph')._outlines);
        await idle(page);
      }
      expect(await map(page, 'return [map.getPaintProperty("globe-water", "background-color"), map.getPaintProperty("globe-countries", "fill-color")]'))
        .toEqual([palettes[theme].water, palettes[theme].accent]);
      await expect.poll(() => highlightAt(page, ...CANADA)).toBe('CA');
      // The globe, its canvas and its controls fit the view pane and the
      // viewport. (The explorer's header row overflows 390px on main too.)
      const pane = await page.locator('#graph').boundingBox();
      for (const selector of ['.globe-view', '.maplibregl-canvas', '.map-count', '.maplibregl-ctrl-zoom-in']) {
        const box = await page.locator(selector).boundingBox();
        expect(box.x, selector).toBeGreaterThanOrEqual(pane.x - 1);
        expect(box.x + box.width, selector).toBeLessThanOrEqual(Math.min(width, pane.x + pane.width) + 1);
      }
      expect((await page.locator('.globe-view').boundingBox()).width).toBeGreaterThan(200);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({ path: `../.tmp/globe-${theme}-${width}.png` });
    }
  }
});

test('missing outlines keep places; a bad setting underlines its value', async ({ page }) => {
  await page.route('**/data/countries-110m.geojson', (route) => route.fulfill({ status: 404 }));
  await globe(page);
  await expect(page.locator('.map-notice')).toHaveText('Country outlines unavailable. Your places are still shown.');
  await expect.poll(() => map(page, 'return map.getLayer("zega-nodes") ? map.queryRenderedFeatures({ layers: ["zega-nodes"] }).length : -1')).toBeGreaterThan(0);
  await setEditor(page, 'schema', 'type Country { iso: String<iso2> }\ndisplay { globe(@zoom: 30) }');
  await expect.poll(() => page.evaluate(() => window.monaco.editor.getModelMarkers({ owner: 'zega' }).map(({ message, startLineNumber, startColumn, endColumn }) => ({ message, startLineNumber, startColumn, endColumn })))).toEqual([{
    message: '@zoom must be a number from 0 to 22\n\n1.5 shows the whole globe; the globe becomes the flat map near 12', startLineNumber: 2, startColumn: 24, endColumn: 26,
  }]);
  await setEditor(page, 'schema', SCHEMA.replace('iso: "JP"', 'iso: "jp"'));
  await page.locator('#btn-run').click();
  await expect.poll(async () => page.evaluate(() => window.monaco.editor.getEditors().find((e) => e.getDomNode()?.closest('#output')).getValue())).toContain('Country.iso must be String<iso2>');
});
