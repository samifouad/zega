import { test, expect } from './offline.js';
import { mkdir } from 'node:fs/promises';

// zegadb/zega#6: a button next to the settings gear expands the graph into a
// near-full-viewport modal; the button or Escape closes it. The graph is not
// re-rendered, so its layout, zoom and selection carry over, and the view
// re-fits to the space it is given.

async function ready(page) {
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  await expect(page.locator('#graph g[data-node]').first()).toBeVisible();
}

// The force layout runs for a few seconds; measure the picture once it rests.
const settle = (page) => page.waitForFunction(() => {
  const sim = document.querySelector('#graph')._sim;
  return !sim || sim.alpha() < sim.alphaMin();
}, null, { timeout: 30_000 });

const expandButton = (page) => page.locator('#graph .graph-expand');
const graph = (page) => page.locator('#graph');
const nodes = (page) => page.locator('#graph g[data-node]');

// The part of the screen the drawn nodes cover, in CSS pixels.
function nodeSpread(page) {
  return page.locator('#graph g[data-node]').evaluateAll((els) => {
    const rects = els.map((el) => el.getBoundingClientRect()).filter((r) => r.width > 0);
    const left = Math.min(...rects.map((r) => r.left)), right = Math.max(...rects.map((r) => r.right));
    const top = Math.min(...rects.map((r) => r.top)), bottom = Math.max(...rects.map((r) => r.bottom));
    return { left, right, top, bottom, width: right - left, height: bottom - top };
  });
}

const box = (locator) => locator.evaluate((el) => {
  const r = el.getBoundingClientRect();
  return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
});

test('the expand button sits beside the gear and opens a near-full-viewport modal', async ({ page }) => {
  await ready(page);
  const gear = await box(page.locator('#graph .graph-gear'));
  const button = await box(expandButton(page));
  expect(Math.abs(button.top - gear.top)).toBeLessThanOrEqual(2);
  expect(gear.left - button.right).toBeGreaterThanOrEqual(0);
  expect(gear.left - button.right).toBeLessThanOrEqual(12);
  await expect(expandButton(page)).toHaveAccessibleName('Expand graph');

  await settle(page);
  const closedSvg = await box(page.locator('#graph .graph-wrap > svg'));
  const closedSpread = await nodeSpread(page);
  await expandButton(page).click();

  await expect(graph(page)).toHaveClass(/graph-expanded/);
  await expect(page.getByRole('dialog', { name: 'Graph, expanded' })).toBeVisible();
  await expect(expandButton(page)).toHaveAccessibleName('Close expanded graph');
  const viewport = page.viewportSize();
  const modal = await box(graph(page));
  expect(modal.width).toBeGreaterThanOrEqual(viewport.width - 50);
  expect(modal.height).toBeGreaterThanOrEqual(viewport.height - 50);
  // It covers the editors: the element at the centre of the screen is the graph.
  const centre = await page.evaluate(({ width, height }) => !!document.elementFromPoint(width / 2, height / 2)?.closest('#graph'), viewport);
  expect(centre).toBe(true);

  // Re-fit: the drawing grows into the new space instead of keeping its old size.
  const openSvg = await box(page.locator('#graph .graph-wrap > svg'));
  expect(openSvg.width).toBeGreaterThan(closedSvg.width);
  expect(openSvg.width).toBeGreaterThanOrEqual(modal.width - 40);
  expect(openSvg.height).toBeGreaterThan(closedSvg.height * 1.5);
  await expect.poll(async () => (await nodeSpread(page)).width).toBeGreaterThan(closedSpread.width * 1.3);
  const spread = await nodeSpread(page);
  expect(spread.left).toBeGreaterThanOrEqual(modal.left);
  expect(spread.right).toBeLessThanOrEqual(modal.right);
  expect(spread.top).toBeGreaterThanOrEqual(modal.top);
  expect(spread.bottom).toBeLessThanOrEqual(modal.bottom);
});

test('the button and Escape close it, and focus returns to the button', async ({ page }) => {
  await ready(page);
  const closedSvg = await box(page.locator('#graph .graph-wrap > svg'));

  await expandButton(page).click();
  await expect(graph(page)).toHaveClass(/graph-expanded/);
  await expandButton(page).click();
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
  await expect(page.getByRole('dialog', { name: 'Graph, expanded' })).toHaveCount(0);
  const reclosed = await box(page.locator('#graph .graph-wrap > svg'));
  expect(Math.abs(reclosed.width - closedSvg.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(reclosed.height - closedSvg.height)).toBeLessThanOrEqual(1);

  await expandButton(page).click();
  await expect(graph(page)).toHaveClass(/graph-expanded/);
  await page.keyboard.press('Escape');
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
  await expect(expandButton(page)).toBeFocused();
});

test('Escape closes an open node preview first, then the modal', async ({ page }) => {
  await ready(page);
  await expandButton(page).click();
  await nodes(page).first().focus();
  await page.keyboard.press('Space');
  await expect(page.locator('dialog[open]')).toBeVisible();

  await page.keyboard.press('Escape');
  await expect(page.locator('dialog[open]')).toHaveCount(0);
  await expect(graph(page)).toHaveClass(/graph-expanded/);

  await page.keyboard.press('Escape');
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
});

test('layout, zoom and selection carry over into the modal and back', async ({ page }) => {
  await ready(page);
  const first = nodes(page).first();
  const id = await first.getAttribute('data-node');
  await first.focus();
  await page.keyboard.press('Enter');
  await expect(first).toHaveAttribute('aria-pressed', 'true');
  for (let i = 0; i < 3; i++) await page.locator('#graph [data-zoom="in"]').click();

  const snapshot = () => page.evaluate(() => {
    const container = document.querySelector('#graph');
    return {
      transform: container.querySelector('.viewport').getAttribute('transform'),
      sameRender: container._graph === window.__expandGraph && container.querySelector('svg') === window.__expandSvg,
    };
  });
  await page.evaluate(() => {
    const container = document.querySelector('#graph');
    window.__expandGraph = container._graph;
    window.__expandSvg = container.querySelector('svg');
  });
  const before = await snapshot();

  await expandButton(page).click();
  await expect(graph(page)).toHaveClass(/graph-expanded/);
  // Give a re-fit, if there were one, a chance to run: there must not be one
  // after the reader has zoomed.
  await page.waitForTimeout(300);
  const open = await snapshot();
  expect(open.sameRender).toBe(true);
  expect(open.transform).toBe(before.transform);
  await expect(page.locator(`#graph g[data-node="${id}"]`)).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#graph g[aria-pressed="true"]')).toHaveCount(1);

  await page.keyboard.press('Escape');
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
  await page.waitForTimeout(300);
  const closed = await snapshot();
  expect(closed.sameRender).toBe(true);
  expect(closed.transform).toBe(before.transform);
  await expect(page.locator(`#graph g[data-node="${id}"]`)).toHaveAttribute('aria-pressed', 'true');
});

test('a re-render while expanded stays expanded', async ({ page }) => {
  await ready(page);
  await expandButton(page).click();
  await page.evaluate(async () => {
    const { renderGraph } = await import('/graph.js');
    const container = document.querySelector('#graph');
    renderGraph(container, { nodes: [{ id: 1, labels: ['Note'], name: 'one' }, { id: 2, labels: ['Note'], name: 'two' }], rels: [{ id: 1, from: 1, to: 2, type: 'next' }] });
  });
  await expect(nodes(page)).toHaveCount(2);
  await expect(graph(page)).toHaveClass(/graph-expanded/);
  await expect(expandButton(page)).toHaveAccessibleName('Close expanded graph');
  await page.keyboard.press('Escape');
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
});

test('at 390px the modal fits the screen with nothing off its edges', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page);
  await expandButton(page).click();
  await expect(graph(page)).toHaveClass(/graph-expanded/);
  const modal = await box(graph(page));
  expect(modal.left).toBeGreaterThanOrEqual(0);
  expect(modal.top).toBeGreaterThanOrEqual(0);
  expect(modal.right).toBeLessThanOrEqual(390);
  expect(modal.bottom).toBeLessThanOrEqual(844);
  expect(modal.width).toBeGreaterThanOrEqual(390 - 20);
  const button = await box(expandButton(page));
  expect(button.left).toBeGreaterThanOrEqual(modal.left);
  expect(button.right).toBeLessThanOrEqual(modal.right);
  await expect.poll(async () => {
    const spread = await nodeSpread(page);
    return spread.left >= modal.left && spread.right <= modal.right && spread.top >= modal.top && spread.bottom <= modal.bottom;
  }).toBe(true);
  // Nothing inside the modal runs past it. (The explorer's top bar is wider
  // than a phone with or without this change; the modal is fixed to the
  // viewport, so it does not depend on that.)
  expect(await graph(page).evaluate((el) => el.scrollWidth - el.clientWidth)).toBe(0);
  await expandButton(page).click();
  await expect(graph(page)).not.toHaveClass(/graph-expanded/);
});

test('evidence: closed and expanded at 1440 and 390, light and dark', async ({ page }) => {
  await mkdir('../evidence/graph-expand', { recursive: true });
  for (const [width, height] of [[1440, 1000], [390, 844]]) {
    await page.setViewportSize({ width, height });
    await ready(page);
    await settle(page);
    for (const theme of ['light', 'dark']) {
      await page.evaluate(async (t) => (await import('/theme.js')).applyTheme(t), theme);
      await page.mouse.move(1, 1);
      await page.screenshot({ path: `../evidence/graph-expand/${width}-${theme}-closed.png` });
      await expandButton(page).click();
      await expect(graph(page)).toHaveClass(/graph-expanded/);
      await page.waitForTimeout(400);
      await page.mouse.move(1, 1);
      await page.screenshot({ path: `../evidence/graph-expand/${width}-${theme}-open.png` });
      await page.keyboard.press('Escape');
      await expect(graph(page)).not.toHaveClass(/graph-expanded/);
    }
  }
});
