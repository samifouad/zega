import { test, expect } from './offline.js';
import { readFile } from 'node:fs/promises';

const editorValue = (page, pane) => page.evaluate((pane) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest(`#${pane}`)).getValue(), pane);
async function setEditor(page, pane, value) {
  await page.evaluate(({ pane, value }) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest(`#${pane}`)).setValue(value), { pane, value });
}
async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
}
async function tileFixture(page) {
  const archive = await readFile('tests/fixtures/calgary.pmtiles');
  await page.route('https://tiles.zega.dev/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname !== '/calgary.pmtiles') return route.fulfill({ status: 404 });
    const range = /bytes=(\d+)-(\d+)/.exec(route.request().headers().range || '');
    if (!range) return route.fulfill({ body: archive, contentType: 'application/octet-stream' });
    const start = Number(range[1]), end = Math.min(Number(range[2]), archive.length - 1);
    return route.fulfill({ status: 206, body: archive.subarray(start, end + 1), headers: {
      'content-type': 'application/octet-stream', 'accept-ranges': 'bytes',
      'content-range': `bytes ${start}-${end}/${archive.length}`, 'access-control-allow-origin': '*',
    } });
  });
}

test('view tabs follow the explicit schema and its default; coordinates never infer a view', async ({ page }) => {
  await ready(page);
  await expect(page.getByRole('tab')).toHaveText(['Graph']);
  await setEditor(page, 'query', '');
  await setEditor(page, 'schema', 'type Place { lat: Float lon: Float } display { table { Place } graph: Default }');
  await expect(page.getByRole('tab')).toHaveText(['Table', 'Graph']);
  await expect(page.getByRole('tab', { name: 'Graph' })).toHaveAttribute('aria-selected', 'true');
  await setEditor(page, 'schema', 'type Place { lat: Float lon: Float } display { table graph }');
  await expect(page.getByRole('tab', { name: 'Table' })).toHaveAttribute('aria-selected', 'true');
  await setEditor(page, 'schema', 'type Place { lat: Float lon: Float }');
  await expect(page.getByRole('tab')).toHaveText(['Graph']);
});

test('table sections respect types, sort numbers in both directions, and inspect relationship chips', async ({ page }) => {
  await ready(page);
  await setEditor(page, 'query', '');
  await setEditor(page, 'schema', `${await editorValue(page, 'schema')}\ndisplay { table { Player }: Default graph { Team } }`);
  await expect(page.locator('.table-view section')).toHaveCount(1);
  await expect(page.locator('.table-view section')).toHaveAttribute('data-type', 'Player');
  const salary = page.getByRole('columnheader', { name: 'salary' });
  await salary.getByRole('button').click();
  await expect(salary).toHaveAttribute('aria-sort', 'ascending');
  const values = () => page.locator('.table-view tbody tr').evaluateAll((rows) => rows.map((row) => Number(row.cells[3].textContent)));
  const ascending = await values();
  expect(ascending.length).toBeGreaterThan(20);
  expect(ascending).toEqual([...ascending].sort((a, b) => a - b));
  await salary.getByRole('button').click();
  expect(await values()).toEqual([...ascending].reverse());
  await page.locator('.table-view .chip').filter({ hasText: 'Oilers' }).first().click();
  await expect(page.locator('#node-inspector')).toContainText('Oilers');
  await page.getByRole('tab', { name: 'Graph' }).click();
  await expect(page.locator('#graph g[data-node]')).toHaveCount(7);
});

test('Calgary map draws query markers, attribution, theme, and the shared inspector without network', async ({ page }) => {
  await tileFixture(page);
  await ready(page);
  await page.locator('#btn-calgary').click();
  await expect(page.getByRole('tab')).toHaveText(['Map', 'Table', 'Graph']);
  await expect(page.getByRole('tab', { name: 'Map', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('.map-count')).toHaveText('30 places');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('© OpenStreetMap contributors');
  await expect.poll(() => page.evaluate(() => document.querySelector('#graph')._map?.queryRenderedFeatures({ layers: ['zega-nodes'] }).length || 0)).toBe(30);
  const position = await page.evaluate(() => {
    const map = document.querySelector('#graph')._map;
    const feature = map.queryRenderedFeatures({ layers: ['zega-nodes'] }).find((f) => f.properties.name === 'Calgary Tower');
    return map.project(feature.geometry.coordinates);
  });
  await page.locator('.maplibregl-canvas').click({ position });
  await expect(page.locator('#node-inspector')).toContainText('Calgary Tower');
  await page.locator('#node-inspector button').click();
  await page.locator('#btn-theme').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect.poll(() => page.evaluate(() => document.querySelector('#graph')._map?.queryRenderedFeatures({ layers: ['zega-nodes'] }).length || 0)).toBe(30);
  await setEditor(page, 'query', '{ Place(kind = "cafe") { id name kind lat lon } }');
  await expect(page.locator('.map-count')).toHaveText('4 places');
  await page.reload();
  await expect(page.locator('.map-count')).toHaveText('4 places');
  await expect(page.getByRole('tab')).toHaveText(['Map', 'Table', 'Graph']);
});

test('failed tiles keep the GeoJSON markers and permanent attribution on plain ground', async ({ page }) => {
  await page.route('https://tiles.zega.dev/**', (route) => route.abort());
  await ready(page);
  await page.locator('#btn-calgary').click();
  await expect(page.locator('.map-notice')).toHaveText('Base map unavailable. Your places are still shown.');
  await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('© OpenStreetMap contributors');
  await expect.poll(() => page.evaluate(() => document.querySelector('#graph')._map?.queryRenderedFeatures({ layers: ['zega-nodes'] }).length || 0)).toBe(30);
});

test('bad display produces the engine diagnostic and exact editor underline', async ({ page }) => {
  await ready(page);
  await setEditor(page, 'query', '');
  await setEditor(page, 'schema', 'type Place { lat: String lon: Float }\ndisplay { map { Place }: Default table }');
  await expect.poll(() => editorValue(page, 'output')).toContain('display `map` needs coordinates on type Place');
  await expect.poll(() => page.evaluate(() => window.monaco.editor.getModelMarkers({ owner: 'zega' }).map(({ message, startLineNumber, startColumn, endColumn }) => ({ message, startLineNumber, startColumn, endColumn })))).toEqual([{
    message: 'display `map` needs coordinates on type Place\n\nadd `lat: Float` and `lon: Float` to Place, or remove Place from this view', startLineNumber: 2, startColumn: 17, endColumn: 22,
  }]);
  await expect(page.getByRole('tab')).toHaveCount(0);
});
