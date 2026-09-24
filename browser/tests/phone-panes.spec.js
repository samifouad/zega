import { test, expect } from './offline.js';
import { globe, setEditor } from './globe-helpers.js';

// zegadb/zega#92: on a 390px phone the panes stack in one full-width column,
// view first, then query, output, schema and raw, and the page scrolls down
// instead of sideways. At desktop width nothing changes.

const ORDER = ['view', 'query', 'output', 'schema', 'raw'];
const pane = (page, name) => page.locator(`.pane[data-pane="${name}"]`);
const output = (page) => page.evaluate(() => window.monaco.editor.getEditors()
  .find((editor) => editor.getDomNode()?.closest('#output')).getValue());

async function open(page, width, colorScheme) {
  await page.emulateMedia({ colorScheme });
  await page.setViewportSize({ width, height: width < 600 ? 844 : 1000 });
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  // Pausing clicks a button at the foot of the page, which scrolls to it.
  await page.evaluate(() => window.scrollTo(0, 0));
}

async function expectNoSidewaysScroll(page, view) {
  const [scroll, client] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
  expect(scroll, `${view}: page scrollWidth`).toBeLessThanOrEqual(client);
}

async function inViewport(page, locator) {
  const box = await locator.boundingBox();
  const { height } = page.viewportSize();
  return box.y >= 0 && box.y + box.height <= height;
}

for (const colorScheme of ['light', 'dark']) {
  test(`at 390px in ${colorScheme}, the panes stack full width, view first, with no sideways scroll`, async ({ page }) => {
    await open(page, 390, colorScheme);
    let above = -1;
    for (const name of ORDER) {
      const box = await pane(page, name).boundingBox();
      expect(box.width, `${name} width`).toBeGreaterThanOrEqual(360);
      expect(box.y, `${name} comes after the pane above it`).toBeGreaterThan(above);
      above = box.y + box.height - 1;
    }
    await expectNoSidewaysScroll(page, 'start');
  });

  test(`at 390px in ${colorScheme}, running a query updates the output while it is on screen`, async ({ page }) => {
    await open(page, 390, colorScheme);
    await setEditor(page, 'query', '{ Country(name = "Canada") { name } }');
    await pane(page, 'output').scrollIntoViewIfNeeded();
    expect(await inViewport(page, pane(page, 'output'))).toBe(true);
    // The top bar stays at the top, so Run is at hand wherever the page is.
    await page.getByRole('button', { name: 'Run', exact: true }).click();
    await expect.poll(async () => JSON.parse(await output(page))).toEqual({ name: 'Canada' });
    expect(await inViewport(page, pane(page, 'output'))).toBe(true);
    // Visible and readable: the full width of the phone, not a sliver.
    expect((await pane(page, 'output').boundingBox()).width).toBeGreaterThanOrEqual(360);
    await expect(pane(page, 'output').locator('.view-lines')).toContainText('"Canada"');
  });
}

test('at 390px the long panes fold away and open again', async ({ page }) => {
  await open(page, 390, 'light');
  for (const name of ['output', 'schema', 'raw']) {
    const toggle = pane(page, name).getByRole('button', { name: `Hide ${name}` });
    await toggle.click();
    await expect(pane(page, name).locator('.editor')).toBeHidden();
    await expect(pane(page, name).getByRole('button', { name: `Show ${name}` })).toHaveAttribute('aria-expanded', 'false');
    expect((await pane(page, name).boundingBox()).height).toBeLessThan(60);
    await pane(page, name).getByRole('button', { name: `Show ${name}` }).click();
    await expect(pane(page, name).locator('.editor')).toBeVisible();
    expect((await pane(page, name).boundingBox()).height).toBeGreaterThan(250);
  }
  await expect(pane(page, 'view').locator('.pane-toggle')).toHaveCount(0);
  await expect(pane(page, 'query').locator('.pane-toggle')).toHaveCount(0);
});

test('at 390px every view fits: graph, map, table, vectors and the globe', async ({ page }) => {
  await open(page, 390, 'light');
  for (const sample of ['#btn-tickets', '#btn-calgary']) {
    await page.locator(sample).click();
    await expect(page.locator('#raw-count')).toContainText('nodes');
    for (const tab of await page.getByRole('tab').all()) {
      await tab.click();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
      await expectNoSidewaysScroll(page, `${sample} ${await tab.textContent()}`);
      expect((await pane(page, 'view').boundingBox()).width).toBeGreaterThanOrEqual(360);
    }
  }
  await globe(page, `schema {
  type City { name: String at: Point }
  display {
    globe { City } : Default
    table
  }
}
mutation { City(name: "Calgary" && at: @point(51.05, -114.07)) { name } }`);
  const view = await pane(page, 'view').boundingBox();
  const canvas = await page.locator('.maplibregl-canvas').boundingBox();
  expect(view.width).toBeGreaterThanOrEqual(360);
  expect(canvas.x).toBeGreaterThanOrEqual(view.x);
  expect(canvas.x + canvas.width).toBeLessThanOrEqual(view.x + view.width + 1);
  await expectNoSidewaysScroll(page, 'globe');
});

test('at 1440px the desktop layout is unchanged: two rows, no fold buttons', async ({ page }) => {
  await open(page, 1440, 'light');
  const [schema, view, query, out, raw] = await Promise.all(['schema', 'view', 'query', 'output', 'raw'].map((name) => pane(page, name).boundingBox()));
  // Top row: schema then view; bottom row: query, output, raw, side by side.
  expect(view.y).toBe(schema.y);
  expect(view.x).toBeGreaterThan(schema.x + schema.width);
  expect([out.y, raw.y]).toEqual([query.y, query.y]);
  expect(query.y).toBeGreaterThan(schema.y + schema.height);
  expect(out.x).toBeGreaterThan(query.x + query.width);
  expect(raw.x).toBeGreaterThan(out.x + out.width);
  await expect(page.locator('.pane-toggle')).toHaveCount(3);
  for (const toggle of await page.locator('.pane-toggle').all()) await expect(toggle).toBeHidden();
  // The whole explorer fits the window: no page scroll either way.
  expect(await page.evaluate(() => document.documentElement.scrollHeight <= window.innerHeight)).toBe(true);
});
