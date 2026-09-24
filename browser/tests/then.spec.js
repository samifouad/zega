import { test, expect } from './offline.js';

const source = `schema { type Person { name: String country: String v: Vector<2> at: Point } }
mutation { Person(name: "Alice" && country: "CA" && v: @vector[1,0] && at: @point(0,0)) }
mutation { Person(name: "Bob" && country: "CA" && v: @vector[1,0] && at: @point(0,0)) }
mutation { Person(name: "Carol" && country: "US" && v: @vector[0,1] && at: @point(10,10)) }`;

test('native WASM API carries stages, typed evidence, skip and chaining', async ({ page }) => {
  await page.goto('/');
  const results = await page.evaluate(async (source) => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const db = new ZegaWasm();
    try {
      db.apply(source);
      const expressions = ['common { Person { country } }', 'similar { &v > 0.9 }', 'near { &at <= 0 m }', 'regex { "^(Alice|Bob)$" }', 'findWithout { "Carol" }', 'startsWith { "A" } || endsWith { "ob" }'];
      const stages = expressions.map(expr => JSON.parse(db.run(source, `query { Person { name } } display { skip } then { ${expr} }`)));
      const chained = JSON.parse(db.run(source, 'query { Person } display { skip } then { findWith { "Alice" } } display { skip } then { findWith { "Bob" } }'));
      const diagnostic = JSON.parse(db.check(source, 'query { Person } then { near { &name < 1 km } }'));
      return { stages, chained, diagnostic };
    } finally { db.free(); }
  }, source);
  for (const value of results.stages) {
    expect(value.stages).toHaveLength(1);
    expect(value.stages[0].index).toBe(1);
    expect(value.stages[0].nodes.map(node => node.id)).toEqual([1, 2]);
  }
  expect(results.stages[0].stages[0].couldBeEdges).toEqual([{ from: 1, to: 2, via: { primitive: 'common', fields: ['Person.country'], value: 'CA' } }]);
  expect(results.stages[1].stages[0].couldBeEdges[0].via.score).toBe(1);
  expect(results.stages[2].stages[0].couldBeEdges[0].via.distance).toBe(0);
  expect(results.chained.stages[0].index).toBe(2);
  expect(results.chained.stages[0].nodes).toEqual([]);
  expect(results.diagnostic.text).toContain('must be Point');
});

test('WASM rejects misplaced discovery and unsupported regex, keeps infix filters', async ({ page }) => {
  await page.goto('/');
  const result = await page.evaluate(async (source) => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const db = new ZegaWasm();
    try {
      db.apply(source);
      const errors = ['then { findWith { "Alice" } }', 'query { findWith { "Alice" } }', 'query { Person } then { findWith "Alice" }', 'query { Person } then { regex { "(?=Alice)" } }'].map(query => {
        try { db.run(source, query); return null; } catch (error) { return String(error); }
      });
      return { errors, infix: JSON.parse(db.run(source, 'query { Person(name findWith "lic") { name } }')) };
    } finally { db.free(); }
  }, source);
  expect(result.errors[0]).toContain('preceding query');
  expect(result.errors[1]).toContain('only allowed inside then');
  expect(result.errors[2]).toContain('infix filters need a field');
  expect(result.errors[3]).toContain('lookaround and backreferences');
  expect(result.infix).toEqual([{ name: 'Alice' }]);
});

test('explorer editor executes a pipeline and shows only its visible stages', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-clear').click();
  await page.evaluate(({ source }) => {
    const editor = id => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest(`#${id}`));
    editor('schema').setValue(source);
    editor('query').setValue('query { Person { name } } display { skip } then { common { Person { country } } && findWithout { "Carol" } }');
  }, { source });
  const output = () => page.evaluate(() => window.monaco.editor.getEditors().find(e => e.getDomNode()?.closest('#output')).getValue());
  await page.locator('#btn-run').click();
  await expect.poll(output).toContain('"couldBeEdges"');
  const value = JSON.parse(await output());
  expect(value.stages).toHaveLength(1);
  expect(value.stages[0].index).toBe(1);
  expect(value.stages[0].nodes.map(node => node.id)).toEqual([1, 2]);
  expect(value.stages[0].couldBeEdges[0].via.value).toBe('CA');
});
