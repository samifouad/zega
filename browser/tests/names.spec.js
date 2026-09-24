import { test, expect } from './offline.js';

const schema = 'schema { type Stop { name: String hops: Int cost: Int shape: String road -> Stop[] { hops: Int cost: Int shape: String } } }';
const setup = 'mutation { Stop(name: "A" && hops: 11 && cost: 12 && shape: "circle") { road -> Stop(name: "B" && hops: 21 && cost: 22 && shape: "document") { &hops: 7 &cost: 8 &shape: "map" } } }';
const query = 'query { Stop(name: "A") { hops cost shape road -> Stop { &hops &cost &shape depth: @hops node: @id } } }';

test('editor runs user fields beside built-ins, highlights them, and completes @ names', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-clear').click();
  await page.evaluate(({ schema, setup, query }) => {
    const editor = (id) => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${id}`));
    editor('schema').setValue(`${schema}\n${setup}`);
    editor('query').setValue(query);
  }, { schema, setup, query });
  await page.locator('#btn-run').click();
  await expect.poll(() => page.evaluate(() => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#output')).getValue())).toContain('"depth": 1');
  const result = await page.evaluate(() => {
    const text = window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#output')).getValue();
    return JSON.parse(text);
  });
  expect(result).toEqual({ hops: 11, cost: 12, shape: 'circle', road: [{ hops: 7, cost: 8, shape: 'map', depth: 1, node: 2 }] });
  const tokens = await page.evaluate(() => window.monaco.editor.tokenize('@hops &hops', 'zega-query')[0]);
  expect(tokens.find(token => token.offset === 0).type).toMatch(/^predefined/);
  expect(tokens.find(token => token.offset === 6).type).toMatch(/^variable/);
  await page.evaluate(() => {
    const editor = window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#query'));
    editor.setValue('query { Stop { @h');
    editor.setPosition({ lineNumber: 1, column: 18 });
    editor.focus();
    editor.trigger('test', 'editor.action.triggerSuggest', {});
  });
  await expect(page.locator('.suggest-widget.visible')).toContainText('@hops');
});

test('WASM rejects removed language spellings with migration diagnostics', async ({ page }) => {
  await page.goto('/');
  const reports = await page.evaluate(async () => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const db = new ZegaWasm();
    try {
      const schema = 'type Sample { name: String at?: Point embedding?: Vector<2> road -> Sample[] }';
      return [
        ['{ Sample { &hops } }', '@hops'],
        ['{ Sample { road *path(cost <= 1) -> Sample } }', '@cost'],
        ['{ Sample { road *path(hops <= 1) -> Sample } }', '@hops'],
        ['{ Sample(name STARTS WITH "A") }', 'startsWith'],
        ['{ Sample(name ENDS WITH "A") }', 'endsWith'],
        ['{ Sample(name CONTAINS "A") }', 'findWith'],
        ['{ Sample { id } }', '@id'],
        ['{ Sample { score } }', '@score'],
        ['{ Sample(at: point(0,0)) }', '@point'],
        ['{ Sample(embedding: vector[1,0]) }', '@vector'],
        ['{ Sample(distance(at, @point(0,0)) < 1) }', '@distance'],
        ['{ Sample(similarity(embedding, @vector[1,0]) > 0) }', '@similarity'],
        ['{ Sample(within_box(at, @point(0,0), @point(1,1))) }', '@within_box'],
        ['{ Sample near(embedding, @vector[1,0], 1) }', '@near'],
      ].map(([query, replacement]) => {
        try { db.run(schema, query); return { query, replacement, error: null }; }
        catch (error) { return { query, replacement, error: String(error) }; }
      });
    } finally { db.free(); }
  });
  for (const report of reports) {
    expect(report.error, report.query).toContain(report.replacement);
    expect(report.error, report.query).toContain('^');
  }
});
