import { test, expect } from './offline.js';

const source = 'query{Player{name salary}}';
const formatted = 'query {\n  Player {\n    name\n    salary\n  }\n}\n';
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
