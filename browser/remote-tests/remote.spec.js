import { test, expect } from '../tests/offline.js';
import { tileFixture } from '../tests/map-fixture.js';
import { startRemote } from './stand-in.js';

const TOKEN = 'explorer2-test-token';
const PRODUCTION = 'https://zega-explorer2-api.fly.dev';

// The built explorer-config.json must name the production Fly app; the test
// swaps only the base, so everything else is the deployed bundle.
async function pointAt(page, remote) {
  await page.route('**/explorer-config.json', async (route) => {
    const config = await (await route.fetch()).json();
    expect(config).toEqual({ backend: 'remote', base: PRODUCTION });
    await route.fulfill({ json: { ...config, base: remote.url } });
  });
}

async function withToken(page) {
  await page.addInitScript((token) => {
    if (!sessionStorage.getItem('seeded')) {
      sessionStorage.setItem('seeded', '1');
      localStorage.setItem('zega.remote.token', token);
    }
  }, TOKEN);
}

const editor = (page, pane, value) => page.evaluate(({ pane, value }) => window.monaco.editor.getEditors()
  .find((e) => e.getDomNode()?.closest(`#${pane}`)).setValue(value), { pane, value });
const editorValue = (page, pane) => page.evaluate((pane) => window.monaco.editor.getEditors()
  .find((e) => e.getDomNode()?.closest(`#${pane}`)).getValue(), pane);

async function reloadsAfter(page, action) {
  const loaded = page.waitForEvent('load');
  await action();
  await loaded;
  await ready(page);
}

async function ready(page) {
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45000 });
  await expect(page.locator('#remote-latency')).toHaveAttribute('data-count', /\d+/);
}

let remote;
test.beforeEach(async () => { remote = await startRemote(TOKEN); });
test.afterEach(async () => { await remote.stop(); });

test('asks for the token, sends it as a Bearer header on every request, runs a query and shows latency', async ({ page, request }) => {
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await pointAt(page, remote);
  await page.goto('/');
  await expect(page.locator('#remote-token')).toBeVisible();
  await page.screenshot({ path: '../.tmp/explorer2-token.png' });
  expect(remote.requests).toEqual([]); // nothing reaches the server before a token exists
  await page.locator('#remote-token-input').fill(TOKEN);
  await page.getByRole('button', { name: 'Save token' }).click();
  await ready(page);
  expect(await page.evaluate(() => localStorage.getItem('zega.remote.token'))).toBe(TOKEN);
  await expect(page.locator('.remote-where')).toContainText(`remote · ${new URL(remote.url).host} · Fly shared-cpu-1x`);

  await editor(page, 'schema', 'type Player {\n  name: String\n  salary: Int\n}');
  await editor(page, 'query', 'mutation { Player(name: "Remote row" && salary: 3) { name salary } }');
  await page.locator('#btn-run').click();
  await expect.poll(() => editorValue(page, 'output')).toContain('Remote row');
  await editor(page, 'query', '{ Player { name salary } }');
  await page.locator('#btn-run').click();
  await expect(page.locator('#raw-count')).toContainText('1 node');

  // Each request the page made is timed, and the readout shows last + median.
  const latency = page.locator('#remote-latency');
  await expect(latency).toHaveText(/^last \d+ ms \((GET|POST) \/(graph|zql)\) · median \d+ ms over \d+$/);
  await expect.poll(async () => Number(await latency.getAttribute('data-count')) - remote.requests.length).toBe(0);
  expect(remote.requests.length).toBeGreaterThanOrEqual(4);
  for (const sent of remote.requests) expect(sent.authorization).toBe(`Bearer ${TOKEN}`);
  expect(remote.requests.map((r) => `${r.method} ${r.path}`)).toContain('POST /zql');
  // The stand-in is another origin (another port). Playwright answers CORS
  // preflights itself while routes are installed, so the server's OPTIONS
  // handling is tested with the server (zega-bench-server, branch claude/fly-bench).

  // The row lives on the remote server, not in the page.
  const read = await request.post(`${remote.url}/zql`, {
    headers: { authorization: `Bearer ${TOKEN}` },
    data: { schema: 'type Player { name: String salary: Int }', query: '{ Player { name salary } }' },
  });
  expect((await read.json()).result).toEqual([{ name: 'Remote row', salary: 3 }]);

  const reloaded = page.waitForEvent('load');
  await page.locator('#remote-forget').click();
  await reloaded;
  await expect(page.locator('#remote-token')).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem('zega.remote.token'))).toBeNull();
  expect(errors).toEqual([]);
});

test('a rejected token asks again, and the new one is used', async ({ page }) => {
  await pointAt(page, remote);
  await page.goto('/');
  await page.locator('#remote-token-input').fill('wrong-token');
  await page.getByRole('button', { name: 'Save token' }).click();
  await expect(page.locator('#remote-token')).toContainText('That token was rejected');
  expect(remote.requests.at(-1).authorization).toBe('Bearer wrong-token');
  await page.locator('#remote-token-input').fill(TOKEN);
  await reloadsAfter(page, () => page.getByRole('button', { name: 'Save token' }).click());
  expect(remote.requests.at(-1).authorization).toBe(`Bearer ${TOKEN}`);
  await expect(page.locator('#remote-token')).toHaveCount(0);
});

test('requests go to the Fly app itself: no size or graph in the path', async ({ page }) => {
  await withToken(page);
  await pointAt(page, remote);
  await page.goto('/');
  await ready(page);
  await expect(page.locator('#remote-size')).toHaveCount(0);
  await expect(page.locator('#remote-graph')).toHaveCount(0);
  expect(remote.requests.at(-1).path).toBe('/graph');
  expect(remote.requests.every((r) => !r.path.startsWith('/c/'))).toBe(true);
});

test('a slow first answer says the Machine is waking; load sample fills the empty remote graph', async ({ page, request }) => {
  await tileFixture(page);
  await withToken(page);
  await pointAt(page, remote);
  remote.delayNextMs = 2500;
  await page.goto('/');
  await expect(page.locator('#remote-status')).toHaveText('waking the Machine (it stops when idle)…');
  await expect(page.locator('#remote-status')).toHaveText(/^Machine answered after 2\.\d s \(woke from idle stop\)$/);
  await ready(page);
  await expect(page.locator('#remote-empty')).toBeVisible();

  await page.locator('#remote-sample').click();
  await expect(page.locator('#raw-count')).toContainText('30 nodes', { timeout: 30000 });
  await expect(page.locator('#remote-empty')).toBeHidden();
  await page.screenshot({ path: '../.tmp/explorer2-remote.png' });
  const graph = await request.get(`${remote.url}/graph`, { headers: { authorization: `Bearer ${TOKEN}` } });
  expect((await graph.json()).result.nodes).toHaveLength(30);
});
