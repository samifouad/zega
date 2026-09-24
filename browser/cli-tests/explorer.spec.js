import { test, expect } from '../tests/offline.js';
import { tileFixture } from '../tests/map-fixture.js';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

async function start(directory) {
  const child = spawn(resolve('../.target/debug/zega'), ['explorer', '--port', '0', '--data', directory], { cwd: directory, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (data) => { stderr += data; });
  const url = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { child.kill(); reject(Error(`CLI startup timed out: ${stderr}`)); }, 15000);
    child.once('error', (error) => { clearTimeout(timeout); reject(error); });
    child.once('exit', (code) => { clearTimeout(timeout); reject(Error(`CLI exited ${code}: ${stderr}`)); });
    createInterface({ input: child.stdout }).once('line', (line) => { clearTimeout(timeout); resolve(line); });
  });
  return { url, async stop() { if (child.exitCode !== null) return; const exit = new Promise((resolve) => child.once('exit', resolve)); child.kill(); await exit; } };
}

test('embedded explorer writes through native ZQL and preserves data across reload and restart', async ({ page, request }) => {
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/cli-ui-'));
  let server = await start(directory);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const schema = 'type Player {\n  name: String\n  salary: Int\n}\ndisplay { table: Default graph }';
  const query = '{ Player { name salary } }';
  const read = async () => {
    const response = await request.post(`${server.url}/zql`, { data: { schema, query } });
    expect(response.ok()).toBe(true);
    return (await response.json()).result;
  };
  try {
    await page.goto(server.url);
    await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45000 });
    await expect(page.locator('.conn')).toContainText('native');
    for (const [name, mimeType, text, button] of [
      ['players.csv', 'text/csv', 'Name,Salary\n"Native, CSV",7\n', '#csv-import'],
      ['players.json', 'application/json', '[{"Name":"Native JSON","Salary":8}]', '#csv-merge'],
    ]) {
      await page.locator('#btn-csv').click();
      await page.locator('#csv-file').setInputFiles({ name, mimeType, buffer: Buffer.from(text) });
      await expect(page.locator('#csv-status')).toHaveText(`${name}: 1 rows`);
      await page.evaluate((schema) => window.monaco.editor.getEditors()
        .find((editor) => editor.getDomNode()?.closest('#csv-schema')).setValue(schema), schema);
      await page.locator(button).click();
      await expect.poll(async () => await page.locator('#csv-modal').isHidden() ? 'closed' : await page.evaluate(() => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest('#output')).getValue())).toBe('closed');
    }
    const expected = [{ name: 'Native, CSV', salary: 7 }, { name: 'Native JSON', salary: 8 }];
    expect(await read()).toEqual(expected);
    await expect(page.getByRole('tab')).toHaveText(['Table', 'Graph']);
    await expect(page.locator('.table-view tbody tr')).toHaveCount(2);
    await page.reload();
    await expect(page.locator('#raw-count')).toContainText('2 nodes');
    expect(await read()).toEqual(expected);
    await server.stop();
    server = await start(directory);
    expect(await read()).toEqual(expected);
    await page.goto(server.url);
    await expect(page.locator('#raw-count')).toContainText('2 nodes');
    expect(await read()).toEqual(expected);
    expect(errors).toEqual([]);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});


test('embedded Calgary Point map uses native storage and survives reload and restart', async ({ page, request }) => {
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/cli-map-'));
  let server = await start(directory);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const editorValue = (pane) => page.evaluate((pane) => window.monaco.editor.getEditors()
    .find((editor) => editor.getDomNode()?.closest(`#${pane}`)).getValue(), pane);
  const markers = () => page.evaluate(() => document.querySelector('#graph')._map
    ?.queryRenderedFeatures({ layers: ['zega-nodes'] }).map((feature) => feature.properties.name).sort() || []);
  try {
    await tileFixture(page);
    await page.goto(server.url);
    await expect(page.locator('#query .monaco-editor')).toBeVisible();
    await expect(page.locator('.conn')).toContainText('native');
    await page.locator('#btn-calgary').click();
    await expect(page.getByRole('tab')).toHaveText(['Map', 'Table', 'Graph']);
    await expect(page.getByRole('tab', { name: 'Map', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('#raw-count')).toContainText('30 nodes');
    const schema = await editorValue('schema');
    const query = await editorValue('query');
    const read = async () => {
      const response = await request.post(`${server.url}/zql`, { data: { schema, query } });
      expect(response.ok()).toBe(true);
      return (await response.json()).result;
    };
    const expected = await read();
    expect(expected.length).toBeGreaterThan(1);
    expect(expected.length).toBeLessThan(30);
    expect(expected[0].name).toBe('Calgary Tower');
    expect(expected[0].distance).toBe(0);
    for (const row of expected) {
      expect(row.at.lat).toBeGreaterThan(51);
      expect(row.at.lon).toBeLessThan(-114);
      expect(row.distance).toBeLessThanOrEqual(1500);
    }
    const names = expected.map((row) => row.name).sort();
    await expect.poll(markers).toEqual(names);
    await expect.poll(() => page.evaluate(() => document.querySelector('#graph')._map?.loaded())).toBe(true);
    await expect.poll(() => page.evaluate(() => document.querySelector('#graph')._map
      ?.queryRenderedFeatures().filter((feature) => feature.source === 'basemap').length || 0)).toBeGreaterThan(0);
    await expect(page.locator('.map-notice')).toBeHidden();
    await expect(page.locator('.maplibregl-ctrl-attrib')).toContainText('© OpenStreetMap contributors');
    await page.screenshot({ path: '../.tmp/cli-calgary-map.png', fullPage: true });
    await page.reload();
    await expect.poll(markers).toEqual(names);
    expect(await read()).toEqual(expected);
    await server.stop();
    server = await start(directory);
    expect(await read()).toEqual(expected);
    await page.goto(server.url);
    await expect(page.locator('#raw-count')).toContainText('30 nodes');
    expect(errors).toEqual([]);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});

test('embedded explorer serves the tickets sample in both vector views', async ({ page }) => {
  await mkdir('.tmp', { recursive: true, mode: 0o700 });
  const directory = await mkdtemp(resolve('.tmp/cli-vectors-'));
  const server = await start(directory);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  try {
    await page.goto(server.url);
    await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45000 });
    await expect(page.locator('.conn')).toContainText('native');
    await page.locator('#btn-tickets').click();
    await expect(page.locator('#raw-count')).toContainText('200 nodes', { timeout: 45000 });
    await expect(page.locator('.vector-count')).toHaveText('200 points');
    for (const kind of ['vector2d', 'vector3d']) {
      const tab = page.locator(`[data-view=${kind}]`);
      await tab.click();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
      await expect(page.locator('.vector-stage canvas')).toHaveAttribute('data-points', '200');
      await expect(page.locator('.vector-count')).toHaveText('200 points');
    }
    expect(errors).toEqual([]);
  } finally { await server.stop(); await rm(directory, { recursive: true, force: true }); }
});

test('native storage and WASM checker agree on URL documents and Space preview', async ({page, request}) => {
  await mkdir('.tmp', {recursive:true,mode:0o700});
  const directory=await mkdtemp(resolve('.tmp/cli-shapes-'));
  const server=await start(directory);
  const schema='type Note { name: String scan: String<url> } display { graph { Note(@shape: document, @image: &scan, @size: 2) } }';
  try {
    const inserted=await request.post(`${server.url}/zql`,{data:{schema,query:'mutation { Note(name: "Native document" && scan: "https://example.com/scan.png") { name scan } }'}});
    expect(inserted.ok()).toBe(true);
    const rejected=await request.post(`${server.url}/zql`,{data:{schema,query:'mutation { Note(name: "Rejected" && scan: "broken") }'}});
    expect(rejected.ok()).toBe(false);
    expect((await rejected.json()).error).toContain('must be String<url>');
    await page.goto(server.url);
    await expect(page.locator('#query .monaco-editor')).toBeVisible();
    await page.evaluate((schema) => {
      const editor=(pane) => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${pane}`));
      editor('schema').setValue(schema);editor('query').setValue('{ Note { @id name scan } }');
    },schema);
    await page.locator('#btn-run').click();
    const node=page.locator('g[data-shape="document"]');
    await expect(node).toHaveCount(1);await expect(node).toHaveAttribute('data-size','2');
    await expect(node.locator('.document-fold')).toHaveAttribute('d','M 9 -22 L 18 -13 H 9 Z');
    await node.focus();await page.keyboard.press('Space');
    const dialog=page.getByRole('dialog',{name:'Native document'});
    await expect(dialog).toContainText('https://example.com/scan.png');
    await page.keyboard.press('Escape');await expect(node).toBeFocused();
    await page.reload();await expect(page.locator('g[data-shape="document"]')).toHaveCount(1);
  } finally { await server.stop();await rm(directory,{recursive:true,force:true}); }
});
