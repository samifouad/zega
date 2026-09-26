// Connect to remote graph: the explorer talks to a Zega Cloud graph over
// https://api.zega.dev/g/<id> with the graph's API key, held in memory only.
//
// The browser here really calls https://api.zega.dev: Chromium's resolver
// maps that host to a local HTTPS server that mimics the cloud router
// (key check, /zql, /graph, and CORS for exactly the page's origin), so the
// preflight, the Authorization header and the CORS decision all happen in the
// browser, as in production. Nothing in the page is stubbed.
//
// No Playwright request routing here (tests/offline.js uses it): with routing
// on, Playwright answers CORS preflights itself, and the preflight would never
// be tested. The same local server serves Monaco as cdn.jsdelivr.net, and
// every other host resolves to nothing, so the run stays offline.
import { test, expect, chromium } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { createServer } from 'node:https';
import { join, resolve, sep } from 'node:path';

const GRAPH = 'u7bgg2k9q4xmh3ne5t';
const KEY = 'zk_abcdefghijklmnopqrstuvwxyz234567';
const WRONG_KEY = 'zk_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const PAGE = 'http://127.0.0.1:8787';

const SNAPSHOT = {
  nodes: [
    { id: 1, labels: ['Country'], name: 'Remoteland', flag: '' },
    { id: 2, labels: ['Player'], name: 'Ada Remote', position: 'C', face: '', salary: 1 },
  ],
  rels: [{ id: 1, type: 'born', from: 1, to: 2, props: {} }],
};

const MONACO = resolve('node_modules/monaco-editor/min');
const MONACO_PREFIX = '/npm/monaco-editor@0.52.2/min/';

/** cdn.jsdelivr.net, for Monaco only, from node_modules. */
function serveMonaco(request, response) {
  const url = new URL(request.url, 'https://cdn.jsdelivr.net');
  const path = resolve(MONACO, decodeURIComponent(url.pathname.slice(MONACO_PREFIX.length)));
  if (!url.pathname.startsWith(MONACO_PREFIX) || !path.startsWith(MONACO + sep)) {
    response.writeHead(404);
    return response.end();
  }
  const type = path.endsWith('.js') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : 'application/octet-stream';
  try {
    const body = readFileSync(path);
    response.writeHead(200, { 'content-type': type, 'access-control-allow-origin': '*' });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
}

/** The cloud router, as far as the explorer can see it (zegadb/cloud src/router.ts). */
function fakeRouter(tls) {
  const seen = [];
  const server = createServer(tls, (request, response) => {
    if (request.headers.host?.startsWith('cdn.jsdelivr.net')) return serveMonaco(request, response);
    let body = '';
    request.on('data', (chunk) => { body += chunk; });
    request.on('end', () => {
      seen.push({ method: request.method, url: request.url, headers: request.headers, body });
      const cors = request.headers.origin === PAGE ? { 'access-control-allow-origin': PAGE, vary: 'origin' } : { vary: 'origin' };
      if (request.method === 'OPTIONS') {
        response.writeHead(204, request.headers.origin === PAGE ? {
          ...cors, 'access-control-allow-methods': 'GET, POST, DELETE', 'access-control-allow-headers': 'authorization, content-type',
        } : cors);
        return response.end();
      }
      const send = (status, value) => {
        response.writeHead(status, { ...cors, 'content-type': 'application/json', 'cache-control': 'no-store' });
        response.end(JSON.stringify(value));
      };
      const match = /^\/g\/([^/]+)\/(.+)$/.exec(request.url);
      if (!match) return send(404, { ok: false, code: 'not_found', error: 'Not found.' });
      const auth = request.headers.authorization;
      if (!auth) return send(401, { ok: false, code: 'missing_key', error: 'Send the graph API key as "Authorization: Bearer zk_…".' });
      if (auth !== `Bearer ${KEY}`) return send(401, { ok: false, code: 'invalid_key', error: 'This API key is not valid. It may have been revoked.' });
      if (match[1] !== GRAPH) return send(403, { ok: false, code: 'key_not_for_graph', error: `This API key belongs to a different graph, not "${match[1]}".` });
      if (request.method === 'GET' && match[2] === 'graph') return send(200, { ok: true, result: SNAPSHOT });
      if (request.method === 'POST' && match[2] === 'zql') {
        const { query } = JSON.parse(body);
        return send(200, { ok: true, result: { remote: true, echo: query, name: 'Ada Remote' } });
      }
      return send(404, { ok: false, code: 'not_found', error: 'Not served.' });
    });
  });
  return { server, seen };
}

let browser, router, port;

test.beforeAll(async () => {
  const dir = test.info().outputPath('tls');
  mkdirSync(dir, { recursive: true });
  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=api.zega.dev',
    '-addext', 'subjectAltName=DNS:api.zega.dev,DNS:cdn.jsdelivr.net', '-keyout', join(dir, 'key.pem'), '-out', join(dir, 'cert.pem'),
  ], { stdio: 'ignore' });
  router = fakeRouter({ key: readFileSync(join(dir, 'key.pem')), cert: readFileSync(join(dir, 'cert.pem')) });
  await new Promise((resolve) => router.server.listen(0, '127.0.0.1', resolve));
  port = router.server.address().port;
  const rules = [`MAP api.zega.dev 127.0.0.1:${port}`, `MAP cdn.jsdelivr.net 127.0.0.1:${port}`, 'MAP * ~NOTFOUND', 'EXCLUDE 127.0.0.1'];
  browser = await chromium.launch({ headless: true, args: [`--host-resolver-rules=${rules.join(', ')}`] });
});

test.afterAll(async () => {
  await browser?.close();
  await new Promise((resolve) => router?.server.close(resolve));
});

async function openExplorer() {
  const context = await browser.newContext({ baseURL: PAGE, ignoreHTTPSErrors: true, viewport: { width: 1440, height: 1000 } });
  const page = await context.newPage();
  const logs = [];
  page.on('console', (message) => logs.push(message.text()));
  page.on('pageerror', (error) => logs.push(error.message));
  await page.goto('/');
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  if (await page.locator('#btn-play').textContent() === 'pause') await page.locator('#btn-play').click();
  return { context, page, logs };
}

const editorText = (page, pane) => page.evaluate((id) => window.monaco.editor.getEditors()
  .find((editor) => editor.getDomNode()?.closest(`#${id}`)).getValue(), pane);

/** Every place a page can keep something, plus the URL and the DOM. */
async function everywhere(page, context) {
  const inPage = await page.evaluate(async () => {
    const dump = (store) => Object.fromEntries(Array.from({ length: store.length }, (_, i) => [store.key(i), store.getItem(store.key(i))]));
    const idb = [];
    for (const info of await indexedDB.databases()) {
      const db = await new Promise((resolve, reject) => {
        const open = indexedDB.open(info.name);
        open.onsuccess = () => resolve(open.result);
        open.onerror = () => reject(open.error);
      });
      for (const name of db.objectStoreNames) {
        const all = await new Promise((resolve) => {
          const get = db.transaction(name).objectStore(name).getAll();
          get.onsuccess = () => resolve(get.result);
        });
        idb.push({ db: info.name, store: name, all });
      }
      db.close();
    }
    const caches = [];
    for (const name of await globalThis.caches.keys()) {
      for (const request of await (await globalThis.caches.open(name)).keys()) caches.push(request.url);
    }
    return {
      localStorage: dump(localStorage), sessionStorage: dump(sessionStorage), indexedDB: idb, caches,
      cookie: document.cookie, url: location.href, history: history.state, name: window.name,
      html: document.documentElement.outerHTML,
      inputs: [...document.querySelectorAll('input, textarea')].map((input) => input.value),
    };
  });
  return JSON.stringify({ inPage, storageState: await context.storageState({ indexedDB: true }) });
}

test('connect to a remote graph: query it, disconnect, and the key is never persisted', async () => {
  const { context, page, logs } = await openExplorer();
  const conn = page.locator('.conn');
  const button = page.getByRole('button', { name: 'Connect to remote graph' });
  await expect(conn).toContainText('local · wasm');
  await expect(button).toBeVisible();

  // Malformed input is refused before any request.
  await button.click();
  const dialog = page.locator('#remote-dialog');
  await expect(dialog).toBeVisible();
  await page.screenshot({ path: test.info().outputPath('remote-dialog.png') });
  await page.getByLabel('Graph id').fill(GRAPH);
  await page.getByLabel('API key').fill('not-a-key');
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.locator('#remote-error')).toHaveText('An API key is zk_ followed by 32 characters.');
  expect(router.seen).toEqual([]);

  // A well-formed but wrong key: the router's own answer is shown, and nothing switches.
  await page.getByLabel('API key').fill(WRONG_KEY);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.locator('#remote-error')).toHaveText('This API key is not valid. It may have been revoked.');
  await page.screenshot({ path: test.info().outputPath('remote-dialog-error.png') });
  await expect(conn).toContainText('local · wasm');

  // The right key: connected.
  const start = router.seen.length;
  await page.getByLabel('API key').fill(KEY);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(conn).toHaveText(`connected to ${GRAPH}`);
  await expect(page.getByRole('button', { name: 'Disconnect' })).toBeVisible();
  for (const name of ['Flights', 'Tickets', 'Cities', 'Calgary', 'reset']) {
    await expect(page.getByRole('button', { name, exact: true })).toBeHidden();
  }
  await expect(page.locator('#raw-count')).toHaveText('2 nodes · 1 edges');
  await expect.poll(() => editorText(page, 'raw')).toContain('Ada Remote');

  // What went over the wire: a real CORS preflight (sent with the first attempt; the browser
  // may cache it), then the key as a bearer, JSON, and no cookie.
  const preflight = router.seen.find((r) => r.method === 'OPTIONS');
  expect(preflight?.headers.origin).toBe(PAGE);
  expect(preflight?.headers['access-control-request-headers']).toContain('authorization');
  const snapshot = router.seen.slice(start).find((r) => r.method === 'GET');
  expect(snapshot.url).toBe(`/g/${GRAPH}/graph`);
  expect(snapshot.headers.authorization).toBe(`Bearer ${KEY}`);
  expect(snapshot.headers.accept).toBe('application/json');
  expect(snapshot.headers.origin).toBe(PAGE);
  expect(snapshot.headers.cookie).toBeUndefined();
  expect(snapshot.headers.referer).toBeUndefined();
  await page.screenshot({ path: test.info().outputPath('remote-connected.png') });

  // A query runs on the remote graph.
  const query = '{ Country(name = "Remoteland") { name } }';
  await page.locator('#query .inputarea').focus();
  await page.keyboard.press('ControlOrMeta+A');
  await page.keyboard.press('Backspace');
  await page.keyboard.insertText(query);
  await page.getByRole('button', { name: 'Run', exact: true }).click();
  const sent = 'Country(name = "Remoteland") { name }'; // as the explorer formats it
  await expect.poll(async () => JSON.parse(await editorText(page, 'output') || 'null')).toMatchObject({ remote: true, echo: expect.stringContaining(sent) });
  const zql = router.seen.filter((r) => r.method === 'POST' && r.url === `/g/${GRAPH}/zql`);
  expect(zql.length).toBeGreaterThan(0);
  expect(JSON.parse(zql.at(-1).body)).toMatchObject({ query: expect.stringContaining(sent), document: false });
  expect(zql.at(-1).headers.authorization).toBe(`Bearer ${KEY}`);
  expect(zql.at(-1).headers['content-type']).toBe('application/json');

  // Connected: the key is nowhere but the RemoteDatabase's private field.
  expect(await everywhere(page, context)).not.toContain(KEY);
  expect(await page.evaluate(() => JSON.stringify(window.__zega))).not.toContain('zk_');

  // Disconnect wipes the key: the same object can no longer make a request.
  await page.evaluate(() => { window.__connected = window.__zega; });
  await page.getByRole('button', { name: 'Disconnect' }).click();
  await expect(conn).toHaveText('local · wasm');
  await expect(page.getByRole('button', { name: 'Connect to remote graph' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Flights', exact: true })).toBeVisible();
  const requests = router.seen.length;
  const after = await page.evaluate(() => window.__connected.refresh().then(() => 'sent', (error) => error.message));
  expect(after).toBe('Disconnected from the remote graph.');
  expect(router.seen.length).toBe(requests);
  expect(await everywhere(page, context)).not.toContain(KEY);

  // Connect again, then reload: nothing is remembered and nothing is sent.
  await page.getByRole('button', { name: 'Connect to remote graph' }).click();
  await page.getByLabel('Graph id').fill(GRAPH);
  await page.getByLabel('API key').fill(KEY);
  const reconnect = router.seen.length;
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(conn).toHaveText(`connected to ${GRAPH}`);
  // Connecting reads the graph, reruns the query pane, and reads the graph again.
  const calls = () => router.seen.slice(reconnect).filter((r) => r.method !== 'OPTIONS').map((r) => `${r.method} ${r.url}`);
  await expect.poll(calls).toEqual([`GET /g/${GRAPH}/graph`, `POST /g/${GRAPH}/zql`, `GET /g/${GRAPH}/graph`]);
  const beforeReload = router.seen.length;
  await page.reload();
  await expect(page.locator('#query .monaco-editor')).toBeVisible({ timeout: 45_000 });
  await expect(conn).toContainText('local · wasm');
  await expect(page.getByRole('button', { name: 'Connect to remote graph' })).toBeVisible();
  expect(router.seen.length).toBe(beforeReload);
  expect(await everywhere(page, context)).not.toContain(KEY);
  await page.getByRole('button', { name: 'Connect to remote graph' }).click();
  await expect(page.getByLabel('API key')).toHaveValue('');

  expect(logs.join('\n')).not.toContain(KEY);
  await context.close();
});

test('a remote clear asks first, and Cancel deletes nothing', async () => {
  const { context, page } = await openExplorer();
  await page.getByRole('button', { name: 'Connect to remote graph' }).click();
  await page.getByLabel('Graph id').fill(GRAPH);
  await page.getByLabel('API key').fill(KEY);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.locator('.conn')).toHaveText(`connected to ${GRAPH}`);
  const dialogs = [];
  page.on('dialog', (dialog) => { dialogs.push(dialog.message()); dialog.dismiss(); });
  await page.getByRole('button', { name: 'clear', exact: true }).click();
  await expect.poll(() => dialogs.length).toBe(1);
  expect(dialogs[0]).toContain(GRAPH);
  await expect.poll(() => editorText(page, 'output')).toContain(`Nothing was deleted from ${GRAPH}.`);
  expect(router.seen.some((r) => r.method === 'DELETE')).toBe(false);
  await context.close();
});

test('a Content-Security-Policy, if the explorer ever sends one, lets it connect to api.zega.dev', async ({ request }) => {
  // Today the explorer sends none (worker.js, index.html), so nothing blocks the connection.
  // A policy added later must keep https://api.zega.dev in connect-src (or default-src).
  const response = await request.get(`${PAGE}/`);
  const html = await response.text();
  const policies = [
    response.headers()['content-security-policy'],
    ...[...html.matchAll(/<meta[^>]+http-equiv=["']content-security-policy["'][^>]*content=["']([^"']*)["']/gi)].map((m) => m[1]),
  ].filter(Boolean);
  for (const policy of policies) {
    const directives = Object.fromEntries(policy.split(';').map((d) => d.trim().split(/\s+/)).filter((d) => d[0]).map(([name, ...values]) => [name.toLowerCase(), values]));
    const sources = directives['connect-src'] ?? directives['default-src'];
    if (!sources) continue;
    expect(sources.some((s) => s === 'https://api.zega.dev' || s === 'https:' || s === '*'), policy).toBe(true);
  }
});
