import { expect } from '@playwright/test';
import { tileFixture } from './map-fixture.js';

// Shared by components.spec.js and scripts/bench-components.mjs: open the
// graph-components demo, reach into its canvas, and run swaps.
export async function openComponents(page, { theme = 'light', history = 100 } = {}) {
  await tileFixture(page);
  await page.goto(`/components/?theme=${theme}&history=${history}`);
  await expect(page.locator('html')).toHaveAttribute('data-ready', 'true', { timeout: 30_000 });
  await page.evaluate(() => window.zegaComponents.canvas.idle());
}
// Run `script` with (z, map, surface, arg): z is window.zegaComponents, map the surface's MapLibre instance.
export const inPage = (page, script, arg) => page.evaluate(async ({ script, arg }) => {
  const z = window.zegaComponents;
  const surface = await z.canvas.surface('map');
  return new Function('z', 'map', 'surface', 'arg', `return (async () => { ${script} })()`)(z, surface?.map, surface, arg);
}, { script, arg });
export const renderer = (page) => inPage(page, `
  const gl = map.painter.context.gl;
  const ext = gl.getExtension('WEBGL_debug_renderer_info');
  return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);`);

const spec = (binding, options) => ({ format: 1, component: 'path-on-map', version: '1', binding: { rows: 'points', path: 'at', ...binding }, options });
export const SPEED_3D = spec({ value: 'speed', label: 'name' }, { view: '3d', bearing: -30, showBuildings: true });
export const DIST_2D = spec({ value: 'dist' }, { view: '2d', lineWidth: 9, colorScale: 'diverging' });
export const SOLID_2D = spec({}, { view: '2d', colorScale: 'solid', lineWidth: 8, showBuildings: false });

/**
 * `n` swaps, alternating a new load (the gallery: each page loads its
 * component) with a restore (the notebook: back to an entry), over three
 * specs, which between them turn the buildings on and off. Returns each
 * swap's time (load/restore called to a frame drawn with the new path) and
 * the buildings states the swaps showed.
 */
export const swaps = (page, n) => page.evaluate(async ({ n, specs }) => {
  const { canvas, result } = window.zegaComponents;
  const times = [];
  const surface = await canvas.surface();
  const buildings = new Set();
  for (let i = 0; i < n; i++) {
    if (i % 2 === 0) await canvas.load(specs[(i / 2) % specs.length], result);
    else await canvas.restore((i * 7) % canvas.history.length);
    times.push(canvas.stats().lastSwapMs);
    buildings.add(surface.buildingsShown());
  }
  return { times, buildings: [...buildings].sort() };
}, { n, specs: [SPEED_3D, DIST_2D, SOLID_2D] });

export function summary(times) {
  const sorted = [...times].sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return { median: at(0.5), p95: at(0.95), max: sorted.at(-1) };
}
/** The page's JS heap after forced collection, in bytes. */
export async function collectedHeap(cdp) {
  for (let i = 0; i < 3; i++) await cdp.send('HeapProfiler.collectGarbage');
  return (await cdp.send('Runtime.getHeapUsage')).usedSize;
}
