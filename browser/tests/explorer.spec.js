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
