import { expect } from '@playwright/test';
import { tileFixture } from './map-fixture.js';

// Shared by the globe specs: drive the explorer to a globe view and reach
// into its map.
export async function setEditor(page, pane, value) {
  await page.evaluate(({ pane, value }) => window.monaco.editor.getEditors().find((editor) => editor.getDomNode()?.closest(`#${pane}`)).setValue(value), { pane, value });
}
export const map = (page, script, arg) => page.evaluate(({ script, arg }) => new Function('map', 'arg', script)(document.querySelector('#graph')._map, arg), { script, arg });
export const globeness = (page) => map(page, 'return map ? map.style.projection.transitionState : -1');
export const idle = (page) => page.evaluate(() => new Promise((resolve) => { const m = document.querySelector('#graph')._map; if (m.loaded() && !m.isMoving()) resolve(); else m.once('idle', resolve); }));
// The highlighted country drawn at a longitude/latitude, or null.
export const highlightAt = (page, lon, lat) => map(page, 'const f = map.queryRenderedFeatures(map.project(arg), { layers: ["globe-countries"] })[0]; return f ? f.properties.iso : null', [lon, lat]);

export async function globe(page, schema) {
  await tileFixture(page);
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible();
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await page.locator('#btn-clear').click();
  await setEditor(page, 'query', '');
  await setEditor(page, 'schema', schema);
  await page.locator('#btn-run').click();
  await expect(page.getByRole('tab', { name: 'Globe' })).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => map(page, 'return !!map && map.loaded() && !!map.getSource("countries")')).toBe(true);
  await page.evaluate(() => document.querySelector('#graph')._outlines);
  await idle(page);
}

// Pixels of the map canvas around a canvas point (CSS px), as [r, g, b] rows,
// decoded from a screenshot inside the page.
export async function patch(page, x, y, r = 6) {
  await page.locator('.maplibregl-canvas').scrollIntoViewIfNeeded();
  const box = await page.locator('.maplibregl-canvas').boundingBox();
  const clip = { x: Math.round(box.x + x - r), y: Math.round(box.y + y - r), width: 2 * r + 1, height: 2 * r + 1 };
  const viewport = page.viewportSize();
  // Off the viewport there are no pixels: a point drawn a world away is simply not seen.
  if (clip.x < 0 || clip.y < 0 || clip.x + clip.width > viewport.width || clip.y + clip.height > viewport.height) return [];
  const png = (await page.screenshot({ clip })).toString('base64');
  return page.evaluate(async (png) => {
    const image = new Image();
    image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.width;
    canvas.height = image.height;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
    const pixels = [];
    for (let i = 0; i < data.length; i += 4) pixels.push([data[i], data[i + 1], data[i + 2]]);
    return pixels;
  }, png);
}
export const hex = (color) => [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
// Whether any pixel of the patch is within `tolerance` per channel of the colour.
export async function hasColor(page, x, y, color, { r = 6, tolerance = 48 } = {}) {
  const want = hex(color);
  return (await patch(page, x, y, r)).some((pixel) => pixel.every((channel, i) => Math.abs(channel - want[i]) <= tolerance));
}
