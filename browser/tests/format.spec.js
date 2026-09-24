import { readFileSync } from 'node:fs';
import { test, expect } from './offline.js';

const source = 'query{Player{name salary}}';
const formatted = 'query {\n  Player { name salary }\n}\n';
const value = page => page.evaluate(() => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#query')).getValue());
async function setSource(page, text = source) {
  await page.evaluate(text => {
    const editor = window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#query'));
    editor.setValue(text); editor.setPosition({ lineNumber: 1, column: 8 }); editor.focus();
  }, text);
}
for (const shortcut of ['Meta+s', 'Control+s']) {
  test(`format on save (${shortcut}) keeps cursor line and persists source`, async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
    await expect(page.locator('#raw-count')).toContainText('50 nodes');
    await setSource(page);
    await page.keyboard.press(shortcut);
    await expect.poll(() => value(page)).toBe(formatted);
    const position = await page.evaluate(() => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#query')).getPosition());
    expect(position.lineNumber).toBe(1);
    await page.reload();
    await expect(page.locator('#query .monaco-editor')).toBeVisible();
    await expect.poll(() => value(page)).toBe(formatted);
  });
}
test('Format action uses WASM, preserves comments, incomplete source and undo', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('50 nodes');
  await setSource(page, '// retain me\n' + source);
  await page.getByRole('button', { name: 'Format', exact: true }).click();
  await expect.poll(() => value(page)).toBe('// retain me\n' + formatted);
  await page.evaluate(() => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#query')).trigger('test', 'undo'));
  await expect.poll(() => value(page)).toBe('// retain me\n' + source);
  await setSource(page, 'query{');
  await page.getByRole('button', { name: 'Format', exact: true }).click();
  expect(await value(page)).toBe('query{');
});

test('JSON import preview keeps source bytes and result panes use canonical WASM layout', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('50 nodes');
  const raw = '[{"name":"\\u0041","salary":1e3},{"name":"B","salary":0.10}]';
  const expected = '[\n  { "name": "\\u0041", "salary": 1e3 },\n  { "name": "B", "salary": 0.10 }\n]\n';
  await page.locator('#btn-csv').click();
  await page.locator('#csv-file').setInputFiles({ name: 'literal.json', mimeType: 'application/json', buffer: Buffer.from(raw) });
  await expect(page.locator('#csv-status')).toContainText('2 rows');
  const pane = id => page.evaluate(id => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${id}`))?.getValue(), id);
  await expect.poll(() => pane('csv-json')).toBe(expected);
  const wasm = await page.evaluate(async raw => (await import('/pkg/zega_wasm.js')).format_json(raw), raw);
  expect(wasm).toBe(expected);
  await page.locator('#csv-close').click();
  await setSource(page, 'query{Player{name salary}}');
  await page.locator('#btn-run').click();
  await expect.poll(() => pane('output')).toContain('salary');
  const output = await pane('output');
  expect(output).toMatch(/\{ "name": "[^"]+", "salary": \d+ \}/);
  expect(output).toBe(await page.evaluate(async text => (await import('/pkg/zega_wasm.js')).format_json(text), output));
});

for (const [pane, golden] of [['schema', 'display-attributes'], ['query', 'then']]) {
  test(`format ${golden} syntax through the editor's WASM formatter`, async ({ page }) => {
    const fixture = extension => readFileSync(new URL(`../../zega/src/fmt/goldens/${golden}.${extension}`, import.meta.url), 'utf8');
    await page.goto('/');
    await expect(page.locator(`#${pane} .monaco-editor`)).toBeVisible({ timeout: 45_000 });
    await expect(page.locator('#raw-count')).toContainText('50 nodes');
    await page.evaluate(({ pane, source }) => {
      const editor = window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${pane}`));
      editor.setValue(source); editor.focus();
    }, { pane, source: fixture('input') });
    await page.keyboard.press('Control+s');
    await expect.poll(() => page.evaluate(pane => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${pane}`)).getValue(), pane)).toBe(fixture('expected'));
  });
}
