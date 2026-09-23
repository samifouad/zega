import { test, expect } from '@playwright/test';

test('Wrangler serves modules and wasm with the correct MIME types', async ({ request }) => {
  for (const [path, type] of [['/', 'text/html'], ['/repl.js', 'text/javascript'], ['/pkg/zega_wasm.js', 'text/javascript'], ['/pkg/zega_wasm_bg.wasm', 'application/wasm']]) {
    const response = await request.get(path);
    expect(response.status()).toBe(200);
    expect(response.headers()['content-type']).toContain(type);
    expect(response.headers()['x-content-type-options']).toBe('nosniff');
    expect(response.headers()['cache-control']).toBe('no-cache');
  }
  const wasm = await request.get('/pkg/zega_wasm_bg.wasm');
  expect(WebAssembly.validate(await wasm.body())).toBe(true);
  for (const path of ['/missing.js', '/pkg/missing.wasm', '/.git/config', '/package.json', '/HANDOFF.md']) {
    expect((await request.get(path)).status()).toBe(404);
  }
  console.log('PASS: HTML, ES modules and wasm MIME types; missing/private files return 404.');
});

test('standalone explorer runs a ZQL mutation and read through the editor', async ({ page }) => {
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const response = await page.goto('/');
  expect(response.status()).toBe(200);
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();

  const output = () => page.evaluate(() => window.monaco.editor.getEditors()
    .find((editor) => editor.getDomNode()?.closest('#output')).getValue());
  const enter = async (zql) => {
    await page.locator('#query .inputarea').focus();
    await page.keyboard.press('ControlOrMeta+A');
    await page.keyboard.press('Backspace');
    await page.keyboard.insertText(zql);
    await expect.poll(() => page.evaluate(() => window.monaco.editor.getEditors()
      .find((editor) => editor.getDomNode()?.closest('#query')).getValue())).toBe(zql);
    await page.getByRole('button', { name: 'Run', exact: true }).click();
  };
  // Mutations do not auto-run: this exercises the actual Run button and wasm.
  const mutation = 'mutation { Country(name: "Extraction smoke" && flag: "") { name } }';
  await enter(mutation);
  await expect.poll(output).toContain('Extraction smoke');
  console.log(`ZQL: ${mutation}\nRESULT: ${await output()}`);

  const query = '{ Country(name = "Extraction smoke") { name flag } }';
  await enter(query);
  await expect.poll(async () => JSON.parse(await output())).toEqual({ name: 'Extraction smoke', flag: '' });
  const result = JSON.parse(await output());
  expect(result).toEqual({ name: 'Extraction smoke', flag: '' });
  await expect(page.locator('#graph')).toContainText('Extraction smoke');
  expect(errors).toEqual([]);
  console.log(`ZQL: ${query}\nRESULT: ${JSON.stringify(result)}\nPASS: headless Chromium; real editor, Run button, wasm and graph; no page errors.`);
});

test('Import UI sends raw CSV and JSON to Rust and inserts every row', async ({ page }) => {
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-clear').click();
  const output = () => page.evaluate(() => window.monaco.editor.getEditors()
    .find((editor) => editor.getDomNode()?.closest('#output')).getValue());
  const files = [
    { name: 'players.csv', mimeType: 'text/csv', buffer: Buffer.from('Name,Salary\r\n"CSV, quoted",42\r\n'), expected: [{ name: 'CSV, quoted', salary: 42 }] },
    { name: 'players.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(Array.from({ length: 151 }, (_, i) => ({ Name: `JSON ${i}`, Salary: i })))), expected: Array.from({ length: 151 }, (_, i) => ({ name: `JSON ${i}`, salary: i })) },
  ];
  for (const { expected, ...file } of files) {
    await page.locator('#btn-csv').click();
    await page.locator('#csv-file').setInputFiles(file);
    await expect(page.locator('#csv-status')).toContainText(`${expected.length} rows`);
    await page.evaluate(() => window.monaco.editor.getEditors()
      .find((editor) => editor.getDomNode()?.closest('#csv-schema'))
      .setValue('type Player {\n  name: String\n  salary: Int\n}'));
    await page.locator('#csv-import').click();
    await expect(page.locator('#csv-modal')).toHaveCount(0);
    await expect.poll(async () => { try { return JSON.parse(await output()); } catch { return null; } }).toEqual(expected);
    await expect(page.locator('#raw-count')).toContainText(`${expected.length} nodes`);
  }
  expect(errors).toEqual([]);
});

test('ZQL editor fetches source text and Rust loads JSON and CSV', async ({ page }) => {
  await page.route('https://fixtures.example/players.json', (route) => route.fulfill({ contentType: 'application/json', body: '[{"Name":"Remote JSON","Salary":7}]' }));
  await page.route('https://fixtures.example/players.csv', (route) => route.fulfill({ contentType: 'text/csv', body: 'Name,Salary\n"Remote, CSV",8\n' }));
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-clear').click();
  await page.evaluate(() => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest('#schema')).setValue('type Player { name: String salary: Int }'));
  for (const [format, name] of [['json', 'Remote JSON'], ['csv', 'Remote, CSV']]) {
    await page.evaluate(({ format }) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest('#query'))
      .setValue(`mutation ${format} ["https://fixtures.example/players.${format}"] { Player(name: $Name && salary: $Salary) { name salary } }`), { format });
    await page.locator('#btn-run').click();
    await expect.poll(() => page.evaluate(() => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest('#output')).getValue())).toContain(name);
  }
  await expect(page.locator('#raw-count')).toContainText('2 nodes');
});
