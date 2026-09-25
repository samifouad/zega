import { mkdir, readFile } from 'node:fs/promises';
import { test, expect } from './offline.js';
import { DIST_2D, SOLID_2D, SPEED_3D, collectedHeap, inPage, openComponents as open, renderer, summary, swaps } from './components-helpers.js';
import { tileFixture } from './map-fixture.js';
import { hasColor } from './globe-helpers.js';
import { palettes } from '../theme.js';
import init, { ZegaWasm } from '../pkg/zega_wasm.js';
import { validate, renderErrors, applyFix } from '../components/validate.js';
import { SETTING_KEYWORDS } from '../components/schema.js';
import { checkContract, component, contracts, surfaceRules } from '../components/registry.js';
import { frozenCopy } from '../components/canvas.js';
import { ARCHIVES } from '../components/surfaces/archives.js';
import CONTRACT_SCHEMA from '../components/schemas/v1/contract.schema.json' with { type: 'json' };
import contract from '../components/path-on-map/contract.json' with { type: 'json' };

// APS 21: graph components. The view-spec validator, the one shared canvas
// (load / flush / history / restore), its storage, and path-on-map on the
// Monaco sample.
const SHOTS = '../.tmp/shots';
const RULES = surfaceRules(contract);

// The sample through the real engine, in Node: the results the validator cases bind against.
async function engine() {
  await init({ module_or_path: await readFile('pkg/zega_wasm_bg.wasm') });
  const db = new ZegaWasm();
  const zql = await readFile(contract.examples.sample, 'utf8');
  const sources = {};
  for (const location of JSON.parse(db.load_locations(zql, true))) sources[location] = await readFile(location, 'utf8');
  db.apply_with_sources(zql, JSON.stringify(sources));
  return (query) => JSON.parse(db.run(zql, query));
}
const spec = (binding, extra = {}) => ({ format: 1, component: 'path-on-map', version: '1', binding: { rows: 'points', path: 'at', ...binding }, ...extra });
const run = (s, result) => validate(s, result, contract, RULES);
const first = (outcome, at) => outcome.errors.find((e) => e.at.startsWith(at));

test.describe('contracts and the view-spec validator', () => {
  let query, result;
  test.beforeAll(async () => { query = await engine(); result = query(contract.examples.zql); });

  test('every registered contract conforms, and settings use exactly the published keyword subset', () => {
    for (const c of contracts()) expect(checkContract(c), c.id).toEqual([]);
    // The keywords a setting may use (contract.schema.json) are the ones schema.js interprets.
    expect(Object.keys(CONTRACT_SCHEMA.$defs.setting.properties).sort()).toEqual([...SETTING_KEYWORDS].sort());
    const broken = structuredClone(contract);
    broken.options.lineWidth.multipleOf = 2; // a keyword the interpreter would silently ignore
    broken.surfaceParams.basemap.enum.push('atlantis'); // not an archive
    broken.surfaceParams.basemap.default = 'atlantis';
    broken.roles.value.relativeTo = 'los'; // names no surface parameter
    expect(checkContract(broken).map((e) => e.at)).toEqual(['options.lineWidth.multipleOf']);
    delete broken.options.lineWidth.multipleOf;
    expect(checkContract(broken).map((e) => [e.at, e.message])).toEqual([
      ['roles.value.relativeTo', 'names no surface parameter: `los`'],
      ['surfaceParams.basemap', `"atlantis" is not a map basemap (it has auto, ${Object.keys(ARCHIVES).join(', ')})`],
    ]);
  });

  test('each contract\'s examples: the valid ones bind, the invalid one gives exactly its error, and its fix repairs it', () => {
    for (const example of contract.examples.valid) expect(run(example.spec, result).errors, example.title).toBeUndefined();
    const [invalid] = contract.examples.invalid;
    const outcome = run(invalid.spec, result);
    expect(outcome.errors[0]).toEqual(invalid.error);
    expect(run(applyFix(invalid.spec, outcome.errors[0].fix), result).ok).toBe(true);
  });

  test('a good spec binds rows, with defaults filled and roles normalised', async () => {
    const csv = (await readFile('samples/monaco-track.csv', 'utf8')).trim().split('\n').slice(1).map((line) => line.split(',').map(Number));
    const outcome = run(contract.examples.valid[0].spec, result);
    expect(outcome.ok).toBe(true);
    expect(outcome.data.groups).toHaveLength(1);
    const [group] = outcome.data.groups;
    expect(group.rows.path).toEqual(csv.map(([, lat, lon]) => [lon, lat]));
    expect(group.rows.value).toEqual(csv.map(([, , , , speed]) => speed));
    expect(group.label).toBe('Circuit de Monaco');
    expect(outcome.spec.options).toEqual({ colorScale: 'sequential', valueUnit: 'km/h', lineWidth: 6, view: '3d', bearing: -30, showBuildings: true });
    expect(outcome.spec.surface).toEqual({ bounds: null, basemap: 'auto' });
    // Grouped: one path per group.
    const many = query('query { Circuit { name points <- TrackPoint order by seq { at } } }');
    const grouped = run(spec({ label: 'name' }, { binding: { groups: '.', rows: 'points', path: 'at', label: 'name' } }), many);
    expect(grouped.ok).toBe(true);
    expect(grouped.data.groups.map((g) => [g.label, g.rows.path.length])).toEqual([['Circuit de Monaco', 333]]);
  });

  test('missing structure or roles are refused with a fix drawn from the result', () => {
    const noRows = run({ format: 1, component: 'path-on-map', version: '1', binding: { path: 'at' } }, result);
    expect(first(noRows, 'binding.rows')).toMatchObject({ code: 'missing-rows', help: 'bind it to `points`', fix: [{ op: 'add', path: '/binding/rows', value: 'points' }] });
    const noPath = run({ format: 1, component: 'path-on-map', version: '1', binding: { rows: 'points' } }, result);
    expect(first(noPath, 'binding.path')).toMatchObject({ code: 'missing-role', message: 'role `path` is required: a Point from each row of `points`', fix: [{ op: 'add', path: '/binding/path', value: 'at' }] });
  });

  test('a role bound to the wrong type is refused against the actual result', () => {
    const outcome = run(spec({ value: 'at' }), result);
    expect(first(outcome, 'binding.value')).toMatchObject({ code: 'type', message: 'role `value` needs a Float, but `at` (row 0) is Point, not a Float', help: 'bind it to `dist` or `speed` or `at.lat`' });
    expect(first(run(spec({ label: 'length' }), result), 'binding.label').message).toBe('`length` is not in the result');
    // A Point the map cannot show: beyond Web Mercator's latitude.
    const polar = structuredClone(result);
    polar.points[5].at.lat = 88;
    expect(first(run(spec({}), polar), 'binding.path').message).toBe(`role \`path\` needs a Point, but \`at\` (row 5) is (88, ${polar.points[5].at.lon}), outside lat ±85.0511 / lon ±180 (a Web Mercator map cannot show it)`);
    expect(first(run(spec({}), { name: 'x', points: [{ at: { lat: 1, lon: 1 } }] }), 'binding.rows').message).toBe('a path needs at least 2 rows; the result has 1');
  });

  test('out-of-range, non-finite and unknown options are refused; nothing else is', () => {
    const outcome = run(spec({}, { options: { lineWidth: 40, view: '4d', colorscale: 'solid' } }), result);
    expect(outcome.errors.map((e) => [e.at, e.code])).toEqual([['options.lineWidth', 'range'], ['options.view', 'option'], ['options.colorscale', 'unknown-option']]);
    expect(first(outcome, 'options.lineWidth')).toMatchObject({ help: 'use a value in [1, 16]', fix: [{ op: 'replace', path: '/options/lineWidth', value: 16 }] });
    expect(first(outcome, 'options.colorscale').fix).toEqual([{ op: 'move', from: '/options/colorscale', path: '/options/colorScale' }]);
    expect(renderErrors(outcome.errors)).toContain('error: 40 is above the maximum 16\n  at: options.lineWidth\n  help: use a value in [1, 16]');
    expect(run(spec({}, { options: { lineWidth: 16, bearing: -180 } }), result).ok).toBe(true);
    expect(first(run(spec({}, { options: { bearing: NaN } }), result), 'options.bearing').message).toBe('NaN is not a finite number');
    // Bounds: ranged, and west < east, south < north.
    expect(run(spec({}, { surface: { bounds: [7.41, 43.72, 7.44, 43.75] } }), result).ok).toBe(true);
    expect(run(spec({}, { surface: { bounds: [10, 10, 0, 0] } }), result).errors.map((e) => e.message)).toEqual(['west (10) must be less than east (0)', 'south (10) must be less than north (0)']);
    expect(first(run(spec({}, { surface: { bounds: [0, -89, 1, 0] } }), result), 'surface.bounds[1]').message).toBe('-89 is below the minimum -85.0511');
    expect(first(run(spec({}, { surface: { basemap: 'atlantis' } }), result), 'surface.basemap').code).toBe('option');
  });

  test('inherited names are never keys: constructor, __proto__ and toString are refused, and nothing is polluted', () => {
    const hostile = JSON.parse('{"format":1,"component":"path-on-map","version":"1","constructor":1,"binding":{"rows":"points","path":"at","constructor":"name","__proto__":"name"},"options":{"toString":1,"__proto__":{"lineWidth":99}},"surface":{"hasOwnProperty":1}}');
    const outcome = run(hostile, result);
    expect(outcome.errors.map((e) => e.at).sort()).toEqual(['constructor']);
    // Past the spec's own shape: every inherited name in the binding, options and surface.
    delete hostile.constructor;
    expect(run(hostile, result).errors.map((e) => e.at).sort()).toEqual(['binding.__proto__', 'binding.constructor', 'options.__proto__', 'options.toString', 'surface.hasOwnProperty']);
    expect(first(run(spec({ label: 'constructor' }), result), 'binding.label').message).toBe('`constructor` is not in the result');
    for (const id of ['constructor', 'toString', '__proto__', 'hasOwnProperty']) expect(component(id), id).toBeUndefined();
    // A format-0 spec is migrated first: its `__proto__` role stays a key, and is refused.
    expect(run(JSON.parse('{"component":"path-on-map","version":"1","binding":{"path":"points[].at","__proto__":"name"}}'), result).errors.map((e) => e.at)).toEqual(['binding.__proto__']);
    expect(({}).lineWidth).toBeUndefined();
    // The resolved spec holds only contract keys.
    const clean = run(JSON.parse('{"format":1,"component":"path-on-map","version":"1","binding":{"rows":"points","path":"at"}}'), result);
    expect(Object.keys(clean.spec.binding)).toEqual(['rows', 'path']);
    expect(Object.hasOwn(frozenCopy(JSON.parse('{"a":1,"__proto__":{"x":1}}')), '__proto__')).toBe(false);
  });

  test('sizes are capped: a 33-step path, a 65-level snapshot', () => {
    const long = Array.from({ length: 33 }, (_, i) => `a${i}`).join('.');
    expect(first(run(spec({ value: long }), result), 'binding.value').code).toBe('pattern');
    expect(run(spec({ value: Array.from({ length: 32 }, (_, i) => `a${i}`).join('.') }), result).errors[0].code).toBe('unresolved');
    let deep = {};
    const top = deep;
    for (let i = 0; i < 100; i++) { deep.a = {}; deep = deep.a; }
    expect(() => frozenCopy(top)).toThrow('nested deeper than 64 levels');
    const ok = frozenCopy({ a: [{ b: 1 }] });
    expect(Object.isFrozen(ok.a[0])).toBe(true);
  });

  test('versions: a format-0 spec is migrated; a newer component minor warns; another major is refused with a fix', () => {
    const old = run({ component: 'path-on-map', version: '1', binding: { path: 'points[].at', value: 'points[].speed', label: 'name' } }, result);
    expect(old.ok).toBe(true);
    expect(old.spec.binding).toEqual({ rows: 'points', path: 'at', value: 'speed', label: 'name' });
    expect(old.warnings.map((w) => w.code)).toEqual(['migrated']);
    expect(run(spec({}, { version: '1.4' }), result).warnings.map((w) => w.code)).toEqual(['newer']);
    expect(first(run(spec({}, { version: '2' }), result), 'version').fix).toEqual([{ op: 'replace', path: '/version', value: '1' }]);
    expect(first(run({ ...spec({}), format: 2 }, result), 'format').code).toBe('const');
  });

  test('the format covers planar frames: a rink heat contract (docs only) binds XY in feet', () => {
    const rink = {
      format: 1, id: 'rink-heat', version: '0.1.0', title: 'Rink heat', category: 'Sports', surface: 'rink', mark: 'heat',
      data: { groups: 'none', description: 'One row per event.' },
      roles: {
        at: { per: 'row', type: 'XY', frame: 'rink', unit: 'ft', minimum: -100, maximum: 100, required: true, description: 'Where.' },
        weight: { per: 'row', type: 'Float', minimum: 0, required: false, description: 'How much.' },
      },
      surfaceParams: { half: { enum: ['full', 'offensive', 'defensive'], default: 'full', description: 'Which part.' } },
      options: { bin: { type: 'number', minimum: 1, maximum: 20, default: 5, description: 'Bin size, ft.' } },
      examples: { valid: [{ title: 'a', spec: {} }, { title: 'b', spec: {} }], invalid: [{ title: 'c', spec: {}, error: {} }] },
    };
    expect(checkContract(rink)).toEqual([]);
    const shots = { shots: [{ xy: { x: 60, y: 10 }, w: 1 }, { xy: { x: 150, y: 0 }, w: 2 }] };
    const outcome = validate({ format: 1, component: 'rink-heat', version: '0', binding: { rows: 'shots', at: 'xy', weight: 'w' } }, shots, rink);
    expect(first(outcome, 'binding.at').message).toBe('role `at` needs an XY, but `xy` (row 1) is (150, 0), outside [-100, 100] ft');
  });
});

// The demo page: the same sample loaded into the wasm engine in the page, the
// example query run, and the result drawn on the shared canvas.
const idle = (page) => page.evaluate(() => window.zegaComponents.canvas.idle());
const state = (page) => inPage(page, `
  const layers = map.getStyle().layers.map((l) => l.id).filter((id) => id.startsWith('path-on-map'));
  return {
    layers, owned: surface.owned(), current: z.canvas.current, entries: z.canvas.history.length,
    pitch: Math.round(map.getPitch()), bearing: Math.round(map.getBearing()),
    width: map.getLayer('path-on-map-line') ? map.getPaintProperty('path-on-map-line', 'line-width') : null,
    segments: map.getSource('path-on-map') ? (await map.getSource('path-on-map').getData()).features.length : 0,
    colours: map.getSource('path-on-map') ? new Set((await map.getSource('path-on-map').getData()).features.map((f) => f.properties.c)).size : 0,
    legend: document.querySelector('.zc-legend-name')?.textContent ?? null,
    buildings: surface.buildingsShown(),
  };`);

test('load swaps the view in place, flush clears it, restore redraws an entry from its snapshot', async ({ page }) => {
  await open(page);
  // The demo's own first load is entry 0 (the contract's first example: speed, 3d).
  const load = (s) => inPage(page, 'await z.canvas.load(arg, z.result); return z.canvas.current;', s);
  await inPage(page, 'map.getLayer("path-on-map-line").__marked = true;');
  expect(await load(DIST_2D)).toBe(1);
  let s = await state(page);
  expect(s).toMatchObject({ entries: 2, current: 1, pitch: 0, bearing: 0, width: 9, segments: 332, legend: 'dist', buildings: true });
  expect(s.colours).toBeGreaterThan(50);
  expect(s.layers).toEqual(['path-on-map-casing', 'path-on-map-line', 'path-on-map-start', 'path-on-map-label']);
  // Same component: updated in place, not rebuilt.
  expect(await inPage(page, 'return map.getLayer("path-on-map-line").__marked === true')).toBe(true);

  expect(await load(SOLID_2D)).toBe(2);
  s = await state(page);
  expect(s).toMatchObject({ width: 8, colours: 1, legend: null, buildings: false });

  // flush: nothing of the component is left; the history stays.
  await inPage(page, 'await z.canvas.flush();');
  s = await state(page);
  expect(s).toMatchObject({ layers: [], owned: { layers: 0, sources: 0 }, current: -1, entries: 3, legend: null });

  // The snapshot is the result as it was: changing the caller's object later changes nothing.
  expect(await inPage(page, 'z.result.points.length = 10; return z.result.points.length')).toBe(10);
  expect(await inPage(page, 'return Object.isFrozen(z.canvas.resultOf(z.canvas.history[0]).points)')).toBe(true);
  // One result, stored once, however many entries point at it.
  expect(await inPage(page, 'return [new Set(z.canvas.history.map((e) => e.resultRef)).size, z.canvas.stats().results]')).toEqual([1, 1]);
  await inPage(page, 'await z.canvas.restore(0);');
  s = await state(page);
  expect(s).toMatchObject({ current: 0, entries: 3, pitch: 60, bearing: -30, width: 6, segments: 332, legend: 'speed (km/h)', buildings: true });

  // Loading after going back branches: the new entry's parent is the restored one.
  await inPage(page, 'z.result.points.length = 0;');
  const branch = await inPage(page, `
    const r = await z.query(z.contract.examples.zql);
    const e = await z.canvas.load(arg, r);
    return { parent: e.parent, first: z.canvas.history[0].id, n: z.canvas.history.length, ordinal: e.n, uuid: /^[0-9a-f-]{36}$/.test(e.id) };`, DIST_2D);
  expect(branch).toEqual({ parent: branch.first, first: branch.first, n: 4, ordinal: 4, uuid: true });

  // A spec that fails validation leaves the canvas and the history as they were.
  const before = await state(page);
  const refused = await inPage(page, 'try { await z.canvas.load(arg, z.result); return null; } catch (e) { return { name: e.name, message: e.message }; }',
    { ...DIST_2D, options: { lineWidth: 99 } });
  expect(refused.name).toBe('ViewSpecError');
  expect(refused.message).toContain('99 is above the maximum 16');
  expect(await state(page)).toEqual(before);
});

test('a draw that fails puts the previous view back and records nothing', async ({ page }) => {
  await open(page);
  const before = await state(page);
  const failed = await inPage(page, `
    const rendered = surface.rendered;
    surface.rendered = () => { surface.rendered = rendered; return Promise.reject(new Error('lost the GPU')); };
    try { await z.canvas.load(arg, z.result); return null; } catch (e) { return e.message; }`, SOLID_2D);
  expect(failed).toBe('lost the GPU');
  expect(await state(page)).toEqual(before); // entry 0 drawn again: speed, 3d, buildings
  // And the next load works.
  expect(await inPage(page, 'await z.canvas.load(arg, z.result); return z.canvas.history.length', SOLID_2D)).toBe(2);
});

test('one canvas, one WebGL context and a flat heap over 200 swaps; swap time within budget', async ({ page }) => {
  test.setTimeout(300_000);
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
  const { times, buildings } = await swaps(page, 200);
  const heapAfter = await collectedHeap(cdp);
  const { median, p95, max } = summary(times);
  const growth = heapAfter - heapBefore;
  const gpu = await renderer(page);
  console.log(`${gpu}\n200 swaps: median ${median.toFixed(1)} ms, p95 ${p95.toFixed(1)} ms, max ${max.toFixed(1)} ms; heap ${(heapBefore / 1e6).toFixed(2)} -> ${(heapAfter / 1e6).toFixed(2)} MB (${growth >= 0 ? '+' : ''}${(growth / 1e3).toFixed(0)} KB)`);

  expect(buildings).toEqual([false, true]); // the swaps turned the buildings off and on
  expect(await page.evaluate(() => window.__glContexts)).toBe(1);
  expect(await page.locator('#stage canvas').count()).toBe(1);
  expect(await layers()).toBe(layersBefore);
  expect(await inPage(page, 'return [z.canvas.history.length, z.canvas.stats().results]')).toEqual([8, 1]);
  // Flat: 200 swaps may not grow the collected heap by more than 1 MB
  // (a result snapshot is ~60 KB; a leak of one per swap would be ~12 MB).
  expect(growth).toBeLessThan(1_000_000);
  // Swap time is load/restore to a frame drawn with the new path. The 100 ms
  // target is for a GPU (scripts/bench-components.mjs --gpu). The headless
  // shell's SwiftShader rasterises each 1440x1000 frame on the CPU (~30 ms a
  // frame), so there the bound is a regression guard on the same work.
  expect(median).toBeLessThan(/SwiftShader/.test(gpu) ? 300 : 100);
});

for (const theme of ['light', 'dark']) {
  test(`the path renders on the map in the ${theme} theme`, async ({ page }) => {
    await mkdir(SHOTS, { recursive: true });
    await open(page, { theme });
    // Solid accent, straight down, so the expected colour is exact.
    await inPage(page, 'await z.canvas.load(arg, z.result);', SOLID_2D);
    await idle(page);
    const sample = await inPage(page, `
      const path = z.canvas.resultOf(z.canvas.history[z.canvas.current]).points.map((p) => [p.at.lon, p.at.lat]);
      return [20, 90, 160, 230, 300].map((i) => { const p = map.project(path[i]); return { x: p.x, y: p.y }; });`);
    const accent = palettes[theme].accent;
    for (const { x, y } of sample) expect(await hasColor(page, x, y, accent, { r: 2, tolerance: 24 }), `accent at ${x},${y}`).toBe(true);
    // Flushed, the same pixels are the basemap, not the accent.
    await inPage(page, 'await z.canvas.flush();');
    await idle(page);
    for (const { x, y } of sample) expect(await hasColor(page, x, y, accent, { r: 2, tolerance: 24 }), `no accent at ${x},${y}`).toBe(false);
    // The first example (speed, 3d) and its 2d twin, for review.
    for (const view of ['3d', '2d']) {
      await inPage(page, 'await z.canvas.load({ ...z.EXAMPLE, options: { ...z.EXAMPLE.options, view: arg } }, z.result);', view);
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
function expectedControls(c, s) {
  const out = [];
  for (const [section, field] of [['surface', 'surfaceParams'], ['options', 'options']]) {
    for (const [key, d] of Object.entries(c[field])) {
      const value = s[section][key];
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
  const edited = await inPage(page, 'const e = z.canvas.history[z.canvas.current]; return { parent: e.parent, view: e.spec.options.view, edit: e.meta.edit, pitch: Math.round(map.getPitch()), sameResult: e.resultRef === z.canvas.history[z.canvas.current - 1].resultRef }');
  expect(edited).toEqual({ parent: before.id, view: '2d', edit: { 'options.view': '2d' }, pitch: 0, sameResult: true });
  await page.locator('[data-param="options.lineWidth"]').evaluate((input) => { input.value = '11'; input.dispatchEvent(new Event('change')); });
  await expect.poll(() => inPage(page, 'return map.getPaintProperty("path-on-map-line", "line-width")')).toBe(11);
  await inPage(page, 'await z.canvas.undo();');
  expect(await inPage(page, 'return [map.getPaintProperty("path-on-map-line", "line-width"), Math.round(map.getPitch())]')).toEqual([6, 0]);
  await inPage(page, 'await z.canvas.undo();');
  expect(await inPage(page, 'return [z.canvas.history[z.canvas.current].id, Math.round(map.getPitch())]')).toEqual([before.id, 60]);

  // A value the contract refuses is explained in the gear and makes no entry.
  const n = await inPage(page, 'return z.canvas.history.length');
  await page.locator('[data-param="surface.bounds"]').fill('[7.44, 43.73, 7.41, 43.75]');
  await page.locator('[data-param="surface.bounds"]').press('Enter');
  await expect(page.locator('.zc-gear-error')).toContainText('west (7.44) must be less than east (7.41)');
  expect(await inPage(page, 'return z.canvas.history.length')).toBe(n);
});

test('the history is kept in localStorage, each result once, and read back entry for entry', async ({ page }) => {
  await open(page);
  await inPage(page, 'await z.canvas.load(arg, z.result);', DIST_2D);
  await page.getByRole('button', { name: 'view settings' }).click();
  await page.locator('[data-param="options.colorScale"]').selectOption({ label: 'solid' });
  await expect.poll(() => inPage(page, 'return z.canvas.history.length')).toBe(3);
  const saved = await inPage(page, 'return JSON.parse(JSON.stringify(z.canvas.history))');
  const keys = await page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith('zega.components.demo:')).map((k) => k.split(':')[1]).sort());
  expect(keys).toEqual(['e', 'e', 'e', 'r']);

  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  const back = await inPage(page, 'return { history: JSON.parse(JSON.stringify(z.canvas.history)), current: z.canvas.current, colours: new Set((await map.getSource("path-on-map").getData()).features.map((f) => f.properties.c)).size }');
  expect(back.history).toEqual(saved);
  expect(back.current).toBe(2); // reopened on the last entry: the solid edit
  expect(back.colours).toBe(1);
  expect(await inPage(page, 'return (await z.canvas.load(arg, z.result)).n', SPEED_3D)).toBe(4);

  // A full localStorage never breaks the canvas, and leaves no orphaned result.
  const outcome = await inPage(page, `
    const { localStorageStore } = await import('/components/storage.js');
    const errors = [];
    const store = localStorageStore('zega.test.full', { onError: (e) => errors.push(e.name) });
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (k, v) { if (k.includes(':e:')) throw new DOMException('full', 'QuotaExceededError'); return setItem.call(this, k, v); };
    let saved;
    try { saved = await store.put({ id: 'x', n: 1, resultRef: 'f'.repeat(64) }, { a: 1 }); } finally { Storage.prototype.setItem = setItem; }
    return { saved, errors, keys: Object.keys(localStorage).filter((k) => k.startsWith('zega.test.full')) };`);
  expect(outcome).toEqual({ saved: false, errors: ['QuotaExceededError'], keys: [] });
});

test('stored entries that cannot be used are dropped and reported; the demo falls back to the example', async ({ page }) => {
  await tileFixture(page);
  await page.goto('/components/');
  await expect(page.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  const good = await inPage(page, 'const e = z.canvas.history[0]; return { entry: JSON.parse(JSON.stringify(e)), result: z.canvas.resultOf(e) }');
  await page.evaluate(({ entry, result }) => {
    const K = 'zega.components.demo';
    localStorage.clear();
    const put = (id, value) => localStorage.setItem(`${K}:e:${id}`, typeof value === 'string' ? value : JSON.stringify(value));
    put('garbage', '{not json');
    put('shape', { id: 'shape', n: 'one' });
    put('orphan', { ...entry, id: 'orphan', resultRef: 'a'.repeat(64) });
    localStorage.setItem(`${K}:r:${'b'.repeat(64)}`, JSON.stringify(result));
    put('tampered', { ...entry, id: 'tampered', resultRef: 'b'.repeat(64) });
    localStorage.setItem(`${K}:r:${entry.resultRef}`, JSON.stringify(result));
    put('badspec', { ...entry, id: 'badspec', spec: { ...entry.spec, options: { lineWidth: 99 } } });
  }, good);
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  const after = await inPage(page, 'return { n: z.canvas.history.length, spec: z.canvas.history[0].spec.binding, notes: z.notes }');
  expect(after.n).toBe(1); // only the example, loaded fresh
  expect(after.spec).toEqual({ rows: 'points', path: 'at', value: 'speed', label: 'name' });
  expect(after.notes.filter((n) => n.startsWith('unreadable entry zega.components.demo:e:garbage'))).toHaveLength(1);
  expect(after.notes.filter((n) => n.startsWith('dropped a stored entry that is not an entry'))).toHaveLength(1);
  expect(after.notes.filter((n) => /its result is missing|does not match its hash|99 is above the maximum 16/.test(n))).toHaveLength(3);
  // Dropped entries are gone from storage too, bar the unreadable one (reported, never parsed); results no entry uses are gone.
  const left = await page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith('zega.components.demo:')).map((k) => k.split(':').slice(1, 3).join(':').slice(0, 12)).sort());
  expect(left.filter((k) => k.startsWith('r:'))).toHaveLength(1);
  expect(left.filter((k) => k.startsWith('e:') && !k.startsWith('e:garbage'))).toHaveLength(1);
});

test('two tabs share one notebook without overwriting each other', async ({ page, context }) => {
  await open(page);
  const other = await context.newPage();
  await tileFixture(other);
  await other.goto('/components/');
  await expect(other.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  // Both tabs load at once.
  await Promise.all([
    inPage(page, 'await z.canvas.load(arg, z.result);', DIST_2D),
    inPage(other, 'await z.canvas.load(arg, z.result);', SOLID_2D),
  ]);
  const ids = (p) => inPage(p, 'return z.canvas.history.map((e) => e.id).sort()');
  await expect.poll(async () => [(await ids(page)).length, (await ids(other)).length]).toEqual([3, 3]);
  expect(await ids(page)).toEqual(await ids(other));
  const stored = await page.evaluate(() => Object.keys(localStorage).filter((k) => k.startsWith('zega.components.demo:e:')).map((k) => k.slice('zega.components.demo:e:'.length)).sort());
  expect(stored).toEqual(await ids(page));
  // Every load from both tabs is there: the first tab's example, the second's (it opened on the first's
  // entry, so it made none of its own), and one load each.
  expect(stored).toHaveLength(3);
});
