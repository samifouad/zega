import { test, expect } from './offline.js';

test('WASM index block narrows rows tested and answers like a scan', async ({ page }) => {
  await page.goto('/');
  const report = await page.evaluate(async () => {
    const { default: init, ZegaWasm } = await import('/pkg/zega_wasm.js');
    await init();
    const types = 'type Player { n: Int name: String salary: Int }';
    const withIndex = `schema { ${types} }\nindex {\n  range Player { salary }\n  text Player { name }\n}\n`;
    const without = `schema { ${types} }\n`;
    const a = new ZegaWasm();
    const b = new ZegaWasm();
    const names = ['Connor McDavid', 'Leon Draisaitl', 'Zach Hyman', 'Evan Bouchard'];
    for (let n = 0; n < 400; n++) {
      const insert = `mutation { Player(n: ${n} && name: ${JSON.stringify(`${names[n % 4]} ${n}`)} && salary: ${n}) { n } }`;
      a.run(withIndex, insert);
      b.run(without, insert);
    }
    const errors = [];
    const rows = [];
    for (const [condition, most] of [
      ['salary >= 395', 5],
      ['salary > 100 && salary < 106', 7],
      ['name CONTAINS "McDavid"', 100],
      ['name STARTS WITH "Leon"', 100],
      ['name ENDS WITH "399"', 1],
    ]) {
      const query = `{ Player(${condition}) { n name salary } }`;
      const before = [a.rows_examined(), b.rows_examined()];
      const left = a.run(withIndex, query);
      const right = b.run(without, query);
      const tested = [a.rows_examined() - before[0], b.rows_examined() - before[1]];
      rows.push(tested);
      if (left !== right) errors.push(`${condition}: results differ`);
      if (tested[0] > most) errors.push(`${condition}: indexed tested ${tested[0]}`);
      if (tested[1] !== 400) errors.push(`${condition}: scan tested ${tested[1]}`);
    }
    const bad = JSON.parse(a.check(`schema { ${types} }\nindex { text Player { salary } }\n`, ''));
    a.free();
    b.free();
    return { errors, rows, diagnostic: bad.diagnostics[0]?.message };
  });
  expect(report.errors).toEqual([]);
  expect(report.diagnostic).toBe('text index needs a String field; Player.salary is Int');
});
