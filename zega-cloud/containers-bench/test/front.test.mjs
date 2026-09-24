// The Worker's front door under Node, with a fake Durable Object binding.
// Node lacks the Workers-only crypto.subtle.timingSafeEqual; Node's own
// crypto.timingSafeEqual has the same contract (equal lengths, constant time).
import assert from 'node:assert/strict';
import { timingSafeEqual } from 'node:crypto';
import { test } from 'node:test';

crypto.subtle.timingSafeEqual ??= (a, b) => timingSafeEqual(a, b);
const { handle } = await import('../src/front.js');

const TOKEN = 'front-test-token';
const EXPLORER2 = 'https://explorer2.zega.dev';

function environment() {
  const calls = [];
  const binding = (size) => ({
    getByName: (graph) => ({
      async fetch(request) {
        calls.push({ size, graph, url: request.url, authorization: request.headers.get('authorization'), body: await request.text() });
        return Response.json({ ok: true, result: { nodes: [], rels: [] } });
      },
    }),
  });
  return { calls, env: { ADMIN_TOKEN: TOKEN, ZEGA_LITE: binding('lite'), ZEGA_BASIC: binding('basic'), ZEGA_STD1: binding('std1') } };
}

const request = (path, { method = 'GET', origin, token, headers = {}, body } = {}) => new Request(`https://zega-containers-bench.deka.workers.dev${path}`, {
  method, body,
  headers: { ...(origin ? { origin } : {}), ...(token ? { authorization: `Bearer ${token}` } : {}), ...headers },
});

test('preflight from explorer2 is answered without a token and allows the explorer headers', async () => {
  const { env, calls } = environment();
  const response = await handle(request('/c/basic/explorer/zql', {
    method: 'OPTIONS', origin: EXPLORER2,
    headers: { 'access-control-request-method': 'POST', 'access-control-request-headers': 'authorization,content-type' },
  }), env);
  assert.equal(response.status, 204);
  assert.equal(response.headers.get('access-control-allow-origin'), EXPLORER2);
  assert.equal(response.headers.get('access-control-allow-methods'), 'GET, POST, DELETE');
  assert.equal(response.headers.get('access-control-allow-headers'), 'authorization, content-type');
  assert.equal(response.headers.get('access-control-max-age'), '7200');
  assert.equal(calls.length, 0, 'a preflight never reaches a container');
});

test('localhost is allowed for development; any other origin is not', async () => {
  const { env } = environment();
  for (const origin of ['http://localhost:8788', 'http://127.0.0.1:8788', 'http://localhost']) {
    const response = await handle(request('/c/basic/g/graph', { method: 'OPTIONS', origin }), env);
    assert.equal(response.status, 204, origin);
    assert.equal(response.headers.get('access-control-allow-origin'), origin);
  }
  for (const origin of ['https://explorer.zega.dev', 'https://evil.example', 'http://explorer2.zega.dev', 'https://explorer2.zega.dev.evil.example', 'http://localhost.evil.example', null]) {
    const response = await handle(request('/c/basic/g/graph', { method: 'OPTIONS', origin }), env);
    assert.equal(response.status, 403, String(origin));
    assert.equal(response.headers.get('access-control-allow-origin'), null);
    const real = await handle(request('/c/basic/g/graph', { origin, token: TOKEN }), env);
    assert.equal(real.status, 200, 'CORS is the browser\'s gate; the token still decides');
    assert.equal(real.headers.get('access-control-allow-origin'), null, String(origin));
  }
});

test('every non-OPTIONS request still needs the admin token, and the 401 is readable by explorer2', async () => {
  const { env, calls } = environment();
  for (const token of [undefined, 'wrong-token', `${TOKEN}x`]) {
    for (const method of ['GET', 'POST', 'DELETE']) {
      const response = await handle(request('/c/basic/explorer/graph', { method, origin: EXPLORER2, token }), env);
      assert.equal(response.status, 401, `${method} ${token}`);
      assert.equal(response.headers.get('access-control-allow-origin'), EXPLORER2);
      assert.deepEqual(await response.json(), { ok: false, error: 'unauthorized' });
    }
  }
  assert.equal(calls.length, 0);
  const unset = await handle(request('/c/basic/explorer/graph', { origin: EXPLORER2, token: TOKEN }), { ...env, ADMIN_TOKEN: undefined });
  assert.equal(unset.status, 500);
});

test('an authorized explorer2 request reaches the right size and graph with its headers and body', async () => {
  const { env, calls } = environment();
  const body = JSON.stringify({ schema: '', query: '{ Item { n } }' });
  const response = await handle(request('/c/std1/sami-test/zql', {
    method: 'POST', origin: EXPLORER2, token: TOKEN, headers: { 'content-type': 'application/json' }, body,
  }), env);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('access-control-allow-origin'), EXPLORER2);
  assert.match(response.headers.get('vary'), /Origin/);
  assert.deepEqual(await response.json(), { ok: true, result: { nodes: [], rels: [] } });
  assert.deepEqual(calls, [{
    size: 'std1', graph: 'sami-test', url: 'https://zega-containers-bench.deka.workers.dev/c/std1/sami-test/zql',
    authorization: `Bearer ${TOKEN}`, body,
  }]);
  const missing = await handle(request('/c/huge/g/zql', { origin: EXPLORER2, token: TOKEN }), env);
  assert.equal(missing.status, 404);
  assert.equal(missing.headers.get('access-control-allow-origin'), EXPLORER2);
});
