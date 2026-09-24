import { test, expect } from './offline.js';

// zegadb/zega#57, #71: at phone width the top bar's buttons ran past the window
// (737px of bar in a 390px screen), so the whole explorer scrolled sideways.
// The same check zega.dev uses: nothing wider than the window, in either theme,
// on the explorer's main views.

const WIDTH = 390;

async function ready(page, colorScheme) {
  await page.emulateMedia({ colorScheme });
  await page.setViewportSize({ width: WIDTH, height: 844 });
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(page.locator('#raw-count')).toContainText('nodes');
  await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme);
}

async function expectNoSidewaysScroll(page, view) {
  const widths = await page.evaluate(() => ({
    page: document.documentElement.scrollWidth,
    window: document.documentElement.clientWidth,
    topbar: document.querySelector('#topbar').scrollWidth,
  }));
  expect(widths.page, `${view}: page scrollWidth`).toBeLessThanOrEqual(widths.window);
  expect(widths.topbar, `${view}: top bar scrollWidth`).toBeLessThanOrEqual(widths.window);
  // Every control in the bar is on screen, not clipped or pushed off the edge.
  for (const control of await page.locator('#topbar a, #topbar button').all()) {
    if (!(await control.isVisible())) continue;
    const box = await control.boundingBox();
    const name = (await control.textContent()).trim();
    expect(box.x, `${view}: ${name} left edge`).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width, `${view}: ${name} right edge`).toBeLessThanOrEqual(widths.window);
  }
}

for (const colorScheme of ['light', 'dark']) {
  test(`at 390px in ${colorScheme}, the top bar fits and nothing scrolls sideways`, async ({ page }) => {
    await ready(page, colorScheme);
    await expectNoSidewaysScroll(page, 'start');
    // Every button the bar had is still there and usable.
    for (const name of ['Tickets', 'Calgary', 'Import', 'Run', 'reset', 'clear']) {
      await expect(page.locator('#topbar').getByRole('button', { name, exact: true })).toBeVisible();
    }
    await expect(page.locator('#topbar').getByRole('link', { name: 'back to zega.dev' })).toBeVisible();

    await page.locator('#btn-tickets').click();
    await expect(page.locator('#raw-count')).toContainText('nodes');
    await expectNoSidewaysScroll(page, 'Tickets');

    await page.locator('#btn-calgary').click();
    await expect(page.locator('#raw-count')).toContainText('nodes');
    await expectNoSidewaysScroll(page, 'Calgary');

    // The theme button switches and the bar still fits in the other theme.
    await page.locator('#btn-theme').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', colorScheme === 'light' ? 'dark' : 'light');
    await expectNoSidewaysScroll(page, 'after switching theme');
  });
}
