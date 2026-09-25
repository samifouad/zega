import { mkdir, readFile } from 'node:fs/promises';
import { test, expect } from './offline.js';
import { DIST_2D, SOLID_2D, SPEED_3D, collectedHeap, inPage, openComponents as open, renderer, summary, swaps } from './components-helpers.js';
import { hasColor } from './globe-helpers.js';
import { palettes } from '../theme.js';
import init, { ZegaWasm } from '../pkg/zega_wasm.js';
import { validate, renderErrors } from '../components/validate.js';
import { check } from '../components/schema.js';
import { contracts } from '../components/registry.js';
import CONTRACT_SCHEMA from '../components/contract.schema.json' with { type: 'json' };
import contract from '../components/path-on-map/contract.json' with { type: 'json' };

// APS 21: graph components. The view-spec validator, the one shared canvas
// (load / flush / history / restore), and path-on-map on the Monaco sample.
const SHOTS = '../.tmp/shots';

// The sample through the real engine, in Node: the result every validator case binds against.
async function monacoResult() {
  await init({ module_or_path: await readFile('pkg/zega_wasm_bg.wasm') });
  const db = new ZegaWasm();
  const zql = await readFile(contract.example.sample, 'utf8');
  const sources = {};
  for (const location of JSON.parse(db.load_locations(zql, true))) sources[location] = await readFile(location, 'utf8');
  db.apply_with_sources(zql, JSON.stringify(sources));
  return JSON.parse(db.run(zql, contract.example.zql));
}
const spec = (over = {}) => ({ component: 'path-on-map', version: '1', binding: { path: 'points[].at', value: 'points[].speed', label: 'name' }, ...over });
const errorAt = (outcome, at) => outcome.errors.find((e) => e.at.startsWith(at));

test.describe('view-spec validator', () => {
  let result;
  test.beforeAll(async () => { result = await monacoResult(); });

  test('every registered contract conforms to contract.schema.json', () => {
    for (const c of contracts()) expect(check(c, CONTRACT_SCHEMA), c.id).toEqual([]);
  });

  test('a good spec binds, with defaults filled and roles normalised', async () => {
    const csv = (await readFile('samples/monaco-track.csv', 'utf8')).trim().split('\n').slice(1).map((line) => line.split(',').map(Number));
    const outcome = validate(contract.example.spec, result, contract);
    expect(outcome.errors).toBeUndefined();
    expect(outcome.ok).toBe(true);
    // Every point, in the sample's order, as [lon, lat]; the value per point; the label.
    expect(outcome.data.path).toEqual(csv.map(([, lat, lon]) => [lon, lat]));
    expect(outcome.data.value).toEqual(csv.map(([, , , , speed]) => speed));
    expect(outcome.data.label).toBe('Circuit de Monaco');
    expect(outcome.spec.options).toEqual({ colorScale: 'sequential', lineWidth: 6, view: '3d', bearing: -30, showBuildings: true });
    expect(outcome.spec.surface).toEqual({ bounds: null, basemap: 'auto' });
  });

  test('a missing required role is refused, naming a field that fits', () => {
    const outcome = validate(spec({ binding: { value: 'points[].speed' } }), result, contract);
    expect(outcome.ok).toBe(false);
    const error = errorAt(outcome, 'binding.path');
    expect(error.code).toBe('missing-role');
    expect(error.help).toBe('bind it to `points[].at`');
  });

  test('a role bound to the wrong type is refused against the actual result', () => {
    const outcome = validate(spec({ binding: { path: 'points[].at', value: 'name' } }), result, contract);
    expect(outcome.ok).toBe(false);
    const error = errorAt(outcome, 'binding.value');
    expect(error.code).toBe('type');
    expect(error.message).toBe('role `value` needs a list of numbers, but `name` holds String');
    expect(error.help).toContain('`points[].speed`');
    // A list read without `[]`, and a field the query did not select.
    const unresolved = validate(spec({ binding: { path: 'points.at', label: 'length' } }), result, contract);
    expect(errorAt(unresolved, 'binding.path').help).toBe('write `points[].at` to read `at` from each item');
    expect(errorAt(unresolved, 'binding.label').message).toBe('`length` is not in the result');
    // A value list that does not pair with the path item for item.
    const short = structuredClone(result);
    short.speeds = short.points.slice(1).map((p) => p.speed);
    expect(errorAt(validate(spec({ binding: { path: 'points[].at', value: 'speeds' } }), short, contract), 'binding.value').code).toBe('length');
  });

  test('an out-of-range or unknown option is refused; nothing else is', () => {
    const outcome = validate(spec({ options: { lineWidth: 40, view: '4d', colorscale: 'solid' } }), result, contract);
    expect(outcome.ok).toBe(false);
    expect(outcome.errors.map((e) => [e.at, e.code])).toEqual([['options.lineWidth', 'range'], ['options.view', 'option'], ['options.colorscale', 'unknown-option']]);
    expect(errorAt(outcome, 'options.lineWidth').help).toBe('use a value in [1, 16]');
    expect(errorAt(outcome, 'options.colorscale').help).toBe('did you mean `colorScale`?');
    expect(renderErrors(outcome.errors)).toContain('error: 40 is above the maximum 16\n  at: options.lineWidth\n  help: use a value in [1, 16]');
    // The edges of the range are valid.
    expect(validate(spec({ options: { lineWidth: 16, bearing: -180 } }), result, contract).ok).toBe(true);
    // The spec's own shape, and its version against the contract's.
    expect(errorAt(validate({ component: 'path-on-map', binding: {} }, result, contract), 'spec').message).toBe('`version` is required');
    expect(errorAt(validate(spec({ version: '2' }), result, contract), 'spec.version').code).toBe('version');
  });
});

// The demo page: the same sample loaded into the wasm engine in the page, the
// example query run, and the result drawn on the shared canvas.
const idle = (page) => page.evaluate(() => window.zegaComponents.canvas.idle());
const state = (page) => inPage(page, `
  const layers = map.getStyle().layers.map((l) => l.id).filter((id) => id.startsWith('path-on-map'));
  const line = map.getLayer('path-on-map-line');
  return {
    layers, owned: surface.owned(), current: z.canvas.current, entries: z.canvas.history.length,
    pitch: Math.round(map.getPitch()), bearing: Math.round(map.getBearing()),
    gradient: line ? !!map.getPaintProperty('path-on-map-line', 'line-gradient') : null,
    width: line ? map.getPaintProperty('path-on-map-line', 'line-width') : null,
    points: map.getSource('path-on-map') ? (await map.getSource('path-on-map').getData()).geometry.coordinates.length : 0,
    legend: document.querySelector('.zc-legend-name')?.textContent ?? null,
    canvases: document.querySelectorAll('#stage canvas').length,
  };`);
test('load swaps the view, flush clears it, restore redraws an entry from its snapshot', async ({ page }) => {
  await open(page);
  // The demo's own first load is entry 0 (the contract's example: speed, 3d).
  const load = (spec) => inPage(page, 'await z.canvas.load(arg, z.result); return z.canvas.current;', spec);
  expect(await load(DIST_2D)).toBe(1);
  let s = await state(page);
  expect(s).toMatchObject({ entries: 2, current: 1, pitch: 0, bearing: 0, width: 9, gradient: true, legend: 'dist', points: 333 });
  expect(s.layers).toEqual(['path-on-map-casing', 'path-on-map-line', 'path-on-map-start']); // no label role bound: no label layer

  expect(await load(SOLID_2D)).toBe(2);
  s = await state(page);
  expect(s).toMatchObject({ gradient: false, width: 8, legend: null });
  expect(await inPage(page, 'return map.getLayoutProperty("surface-buildings-3d", "visibility")')).toBe('none');

  // flush: nothing of the component is left; the history stays.
  await inPage(page, 'await z.canvas.flush();');
  s = await state(page);
  expect(s).toMatchObject({ layers: [], owned: { layers: 0, sources: 0 }, current: -1, entries: 3, legend: null });
  expect(await inPage(page, 'return ["path-on-map", "path-on-map-start"].map((id) => !!map.getSource(id))')).toEqual([false, false]);

  // The snapshot is the result as it was: changing the caller's object later changes nothing.
  expect(await inPage(page, 'z.result.points.length = 10; return z.result.points.length')).toBe(10);
  expect(await inPage(page, 'return Object.isFrozen(z.canvas.history[0].result.points)')).toBe(true);
  await inPage(page, 'await z.canvas.restore(0);');
  s = await state(page);
  expect(s).toMatchObject({ current: 0, entries: 3, pitch: 60, bearing: -30, width: 6, gradient: true, legend: 'speed', points: 333 });
  expect(s.layers).toEqual(['path-on-map-casing', 'path-on-map-line', 'path-on-map-start', 'path-on-map-label']);
  expect(await inPage(page, 'return map.getLayoutProperty("surface-buildings-3d", "visibility")')).toBe('visible');

  // Loading after going back branches: the new entry's parent is the restored one.
  await inPage(page, 'z.result.points.length = 0;'); // the caller's object is now empty
  const branch = await inPage(page, `
    const r = await z.query(z.contract.example.zql);
    const e = await z.canvas.load(arg, r);
    return { parent: e.parent, first: z.canvas.history[0].id, n: z.canvas.history.length };`, DIST_2D);
  expect(branch.parent).toBe(branch.first);
  expect(branch.n).toBe(4);

  // A spec that fails validation leaves the canvas and the history as they were.
  const before = await state(page);
  const refused = await inPage(page, 'try { await z.canvas.load(arg, z.result); return null; } catch (e) { return { name: e.name, message: e.message }; }',
    { ...DIST_2D, options: { lineWidth: 99 } });
  expect(refused.name).toBe('ViewSpecError');
  expect(refused.message).toContain('99 is above the maximum 16');
  expect(await state(page)).toEqual(before);
});

test('one canvas, one WebGL context and a flat heap over 200 swaps; swap time within budget', async ({ page }) => {
  test.setTimeout(300_000);
  // Count every WebGL context the page creates.
  await page.addInitScript(() => {
    window.__glContexts = 0;
    const seen = new WeakSet();
    const original = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (kind, ...rest) {
      const context = original.call(this, kind, ...rest);
      if (context && /webgl/.test(kind) && !seen.has(this)) { seen.add(this); window.__glContexts++; }
      return context;
    };
  });
  await open(page, { history: 8 });
  const cdp = await page.context().newCDPSession(page);
  // Warm: shaders compiled, history at its limit, tiles cached, and V8's
  // optimised code for the swap path in place (a heap-snapshot diff after
  // 300 swaps shows only compiled code and the browser's bounded long-task
  // timing buffer growing — no page objects).
  await swaps(page, 200);
  const layers = () => inPage(page, 'await z.canvas.load(arg, z.result); return map.getStyle().layers.length', SPEED_3D);
  const layersBefore = await layers();
  const heapBefore = await collectedHeap(cdp);
  const times = await swaps(page, 200);
  const heapAfter = await collectedHeap(cdp);
  const { median, p95, max } = summary(times);
  const growth = heapAfter - heapBefore;
  const gpu = await renderer(page);
  console.log(`${gpu}\n200 swaps: median ${median.toFixed(1)} ms, p95 ${p95.toFixed(1)} ms, max ${max.toFixed(1)} ms; heap ${(heapBefore / 1e6).toFixed(2)} -> ${(heapAfter / 1e6).toFixed(2)} MB (${growth >= 0 ? '+' : ''}${(growth / 1e3).toFixed(0)} KB)`);

  expect(await page.evaluate(() => window.__glContexts)).toBe(1);
  expect(await page.locator('#stage canvas').count()).toBe(1);
  expect(await layers()).toBe(layersBefore);
  expect(await inPage(page, 'return z.canvas.history.length')).toBe(8);
  // Flat: 200 swaps may not grow the collected heap by more than 1 MB
  // (one entry's snapshot is ~60 KB; a leak of one per swap would be ~12 MB).
  expect(growth).toBeLessThan(1_000_000);
  // Swap time is load/restore to a frame drawn with the new path. The 100 ms
  // target is for a GPU (scripts/bench-components.mjs --gpu: ~35 ms). The
  // headless shell's SwiftShader rasterises each 1440x1000 frame on the CPU,
  // ~30 ms a frame and 3-4 frames a swap, so there the bound is a regression
  // guard on the same work, not the target.
  expect(median).toBeLessThan(/SwiftShader/.test(gpu) ? 400 : 100);
});

for (const theme of ['light', 'dark']) {
  test(`the path renders on the map in the ${theme} theme`, async ({ page }) => {
    await mkdir(SHOTS, { recursive: true });
    await open(page, { theme });
    // Solid accent, straight down, so the expected colour is exact.
    await inPage(page, 'await z.canvas.load(arg, z.result);', SOLID_2D);
    await idle(page);
    const sample = await inPage(page, `
      const path = z.canvas.history[z.canvas.current].result.points.map((p) => [p.at.lon, p.at.lat]);
      return [20, 90, 160, 230, 300].map((i) => { const p = map.project(path[i]); return { x: p.x, y: p.y }; });`);
    const accent = palettes[theme].accent;
    for (const { x, y } of sample) expect(await hasColor(page, x, y, accent, { r: 2, tolerance: 24 }), `accent at ${x},${y}`).toBe(true);
    // Flushed, the same pixels are the basemap, not the accent.
    await inPage(page, 'await z.canvas.flush();');
    await idle(page);
    for (const { x, y } of sample) expect(await hasColor(page, x, y, accent, { r: 2, tolerance: 24 }), `no accent at ${x},${y}`).toBe(false);
    // The example (speed, 3d) and its 2d twin, for review.
    for (const view of ['3d', '2d']) {
      await inPage(page, 'await z.canvas.load({ ...z.contract.example.spec, options: { ...z.contract.example.spec.options, view: arg } }, z.result);', view);
      await idle(page);
      await page.locator('#stage').screenshot({ path: `${SHOTS}/path-on-map-${view}-${theme}.png` });
    }
  });
}

// The gear: every control comes from the contract's descriptors.
const gearControls = (page) => page.locator('.zc-gear-panel [data-param]').evaluateAll((inputs) => inputs.map((input) => ({
  param: input.dataset.param, tag: input.tagName.toLowerCase(), type: input.type,
  min: input.min || null, max: input.max || null, value: input.type === 'checkbox' ? input.checked : input.value,
  options: input.tagName === 'SELECT' ? [...input.options].map((o) => JSON.parse(o.value)) : null,
})));
// What the gear must show for a set of descriptors and values, derived here from the schema rules alone.
function expectedControls(contract, spec) {
  const out = [];
  for (const [section, field] of [['surface', 'surfaceParams'], ['options', 'options']]) {
    for (const [key, d] of Object.entries(contract[field])) {
      const value = spec[section][key];
      const types = [d.type].flat();
      if (d.enum) out.push({ param: `${section}.${key}`, tag: 'select', type: 'select-one', min: null, max: null, value: JSON.stringify(value), options: d.enum });
      else if (types.length === 1 && types[0] === 'boolean') out.push({ param: `${section}.${key}`, tag: 'input', type: 'checkbox', min: null, max: null, value, options: null });
      else if (types.length === 1 && /number|integer/.test(types[0])) out.push({ param: `${section}.${key}`, tag: 'input', type: d.minimum !== undefined && d.maximum !== undefined ? 'range' : 'number', min: String(d.minimum), max: String(d.maximum), value: String(value), options: null });
      else out.push({ param: `${section}.${key}`, tag: 'input', type: 'text', min: null, max: null, value: JSON.stringify(value), options: null });
    }
  }
  return out;
}

test('the gear shows exactly the contract\'s parameters; an edit is a new, undoable entry', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'view settings' }).click();
  const shown = await inPage(page, 'return z.canvas.history[z.canvas.current].spec');
  expect(await gearControls(page)).toEqual(expectedControls(contract, shown));

  // Generated, not hand-written: a parameter the contract starts advertising gets a control.
  await inPage(page, `
    z.contract.options.trail = { type: 'integer', minimum: 0, maximum: 3, default: 2, description: 'test-only' };
    await z.canvas.restore(z.canvas.current);`);
  const controls = await gearControls(page);
  expect(controls.find((c) => c.param === 'options.trail')).toEqual({ param: 'options.trail', tag: 'input', type: 'range', min: '0', max: '3', value: '2', options: null });
  await inPage(page, 'delete z.contract.options.trail; await z.canvas.restore(z.canvas.current);');
  expect((await gearControls(page)).map((c) => c.param)).toEqual(expectedControls(contract, shown).map((c) => c.param));

  // An edit: view 3d -> 2d through the gear.
  const before = await inPage(page, 'return { id: z.canvas.history[z.canvas.current].id, n: z.canvas.history.length }');
  await page.locator('[data-param="options.view"]').selectOption({ label: '2d' });
  await expect.poll(() => inPage(page, 'return z.canvas.history.length')).toBe(before.n + 1);
  const edited = await inPage(page, 'const e = z.canvas.history[z.canvas.current]; return { parent: e.parent, view: e.spec.options.view, edit: e.meta.edit, pitch: Math.round(map.getPitch()), same: e.result === z.canvas.history[z.canvas.current - 1].result, points: e.result.points.length }');
  expect(edited).toEqual({ parent: before.id, view: '2d', edit: { 'options.view': '2d' }, pitch: 0, same: false, points: 333 });
  // A range: lineWidth.
  await page.locator('[data-param="options.lineWidth"]').evaluate((input) => { input.value = '11'; input.dispatchEvent(new Event('change')); });
  await expect.poll(() => inPage(page, 'return map.getPaintProperty("path-on-map-line", "line-width")')).toBe(11);
  // Undo twice: back to the view before both edits.
  await inPage(page, 'await z.canvas.undo();');
  expect(await inPage(page, 'return [map.getPaintProperty("path-on-map-line", "line-width"), Math.round(map.getPitch())]')).toEqual([6, 0]);
  await inPage(page, 'await z.canvas.undo();');
  expect(await inPage(page, 'return [z.canvas.history[z.canvas.current].id, Math.round(map.getPitch())]')).toEqual([before.id, 60]);

  // A value the contract refuses is explained in the gear and makes no entry.
  const n = await inPage(page, 'return z.canvas.history.length');
  await page.locator('[data-param="surface.bounds"]').fill('[7.41, 43.73, 7.43]');
  await page.locator('[data-param="surface.bounds"]').press('Enter');
  await expect(page.locator('.zc-gear-error')).toContainText('needs at least 4 items, got 3');
  expect(await inPage(page, 'return z.canvas.history.length')).toBe(n);
});

test('the history is kept in localStorage and read back, entry for entry', async ({ page }) => {
  await open(page);
  await inPage(page, 'await z.canvas.load(arg, z.result);', DIST_2D);
  await page.getByRole('button', { name: 'view settings' }).click();
  await page.locator('[data-param="options.colorScale"]').selectOption({ label: 'solid' });
  await expect.poll(() => inPage(page, 'return z.canvas.history.length')).toBe(3);
  const saved = await inPage(page, 'return JSON.parse(JSON.stringify(z.canvas.history))');

  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  const back = await inPage(page, 'return { history: JSON.parse(JSON.stringify(z.canvas.history)), current: z.canvas.current, gradient: !!map.getPaintProperty("path-on-map-line", "line-gradient") }');
  expect(back.history).toEqual(saved);
  expect(back.current).toBe(2); // reopened on the last entry: the solid, 2d edit
  expect(back.gradient).toBe(false);
  // Ids continue after the stored ones.
  expect(await inPage(page, 'return (await z.canvas.load(arg, z.result)).id', SPEED_3D)).toBe(Math.max(...saved.map((e) => e.id)) + 1);

  // A full localStorage never breaks the canvas: the entry is drawn and kept in memory, and the failure reported.
  const outcome = await inPage(page, `
    const { localStorageStore } = await import('/components/storage.js');
    const errors = [];
    const store = localStorageStore('zega.test.full', { onError: (e) => errors.push(e.name) });
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = () => { throw new DOMException('full', 'QuotaExceededError'); };
    try { await store.put({ id: 1 }); } finally { Storage.prototype.setItem = setItem; }
    return { errors, listed: (await store.list()).length };`);
  expect(outcome).toEqual({ errors: ['QuotaExceededError'], listed: 0 });
});
